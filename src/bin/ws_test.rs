use anyhow::{Context, Result};
use base64::Engine;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleRate, StreamConfig};
use futures_util::{SinkExt, StreamExt};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite;
use tungstenite::Message;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("ws_test=debug,info")
        .init();

    let args: Vec<String> = std::env::args().collect();
    let api_key =
        parse_arg(&args, "--api-key").context("Usage: ws_test --api-key <KEY> [--language en]")?;
    let language = parse_arg(&args, "--language").unwrap_or_else(|_| "en".to_string());

    // --- Mic setup ---
    let host = cpal::default_host();
    let device = host
        .default_input_device()
        .context("No input device available")?;
    let supported = device.default_input_config()?;
    let native_rate = supported.sample_rate().0;
    let native_channels = supported.channels() as usize;

    println!(
        "[info] Mic: {:?} {}Hz {}ch",
        device.name().unwrap_or_default(),
        native_rate,
        native_channels
    );

    let config = StreamConfig {
        channels: native_channels as u16,
        sample_rate: SampleRate(native_rate),
        buffer_size: cpal::BufferSize::Default,
    };

    let needs_resample = native_rate != 16000;
    let needs_downmix = native_channels > 1;
    let resample_ratio = if needs_resample {
        16000.0 / native_rate as f64
    } else {
        1.0
    };

    let (audio_tx, mut audio_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let resample_state = Arc::new(Mutex::new((0.0_f64, 0.0_f32)));

    let stream = device.build_input_stream(
        &config,
        move |data: &[f32], _: &cpal::InputCallbackInfo| {
            let mono: Vec<f32> = if needs_downmix {
                data.chunks(native_channels)
                    .map(|frame| frame.iter().sum::<f32>() / native_channels as f32)
                    .collect()
            } else {
                data.to_vec()
            };

            let resampled = if needs_resample {
                resample(&mono, resample_ratio, &resample_state)
            } else {
                mono
            };

            // f32 → i16 little-endian PCM bytes
            let bytes: Vec<u8> = resampled
                .iter()
                .flat_map(|&s| {
                    let clamped = s.clamp(-1.0, 1.0);
                    ((clamped * 32767.0) as i16).to_le_bytes()
                })
                .collect();

            let _ = audio_tx.send(bytes);
        },
        |err| eprintln!("[error] Audio capture: {}", err),
        None,
    )?;
    stream.play()?;

    if needs_resample {
        println!(
            "[info] Resampling {}Hz → 16000Hz",
            native_rate
        );
    }

    // --- WebSocket connect ---
    let url = format!(
        "wss://api.elevenlabs.io/v1/speech-to-text/realtime\
         ?model_id=scribe_v2_realtime\
         &language_code={}\
         &audio_format=pcm_16000\
         &commit_strategy=manual",
        language
    );

    let request = tungstenite::http::Request::builder()
        .uri(&url)
        .header("xi-api-key", &api_key)
        .header("Host", "api.elevenlabs.io")
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .context("Failed to build WebSocket request")?;

    println!("[info] Connecting to ElevenLabs realtime...");
    let (ws, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("WebSocket connection failed")?;

    let (mut ws_write, mut ws_read) = ws.split();
    println!("[info] Connected! Streaming... press Ctrl+C to stop\n");

    // --- Receiver task: print everything from the server ---
    let recv_handle = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_read.next().await {
            if let Message::Text(text) = msg {
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&text) {
                    match parsed["message_type"].as_str() {
                        Some("session_started") => {
                            let sid = parsed["session_id"].as_str().unwrap_or("?");
                            println!("[session] started: {}", sid);
                        }
                        Some("partial_transcript") => {
                            let t = parsed["text"].as_str().unwrap_or("");
                            if !t.is_empty() {
                                println!("[partial] {}", t);
                            }
                        }
                        Some("committed_transcript") => {
                            let t = parsed["text"].as_str().unwrap_or("");
                            println!("[final]   {}", t);
                        }
                        Some("input_error") => {
                            let code = parsed["code"].as_str().unwrap_or("?");
                            let message = parsed["message"].as_str().unwrap_or("");
                            eprintln!("[error]   {} - {}", code, message);
                        }
                        other => {
                            println!("[ws]      {:?}: {}", other, text);
                        }
                    }
                } else {
                    println!("[ws-raw]  {}", text);
                }
            }
        }
        println!("[info] WebSocket closed by server.");
    });

    // --- Main loop: forward mic audio to WS, stop on Ctrl+C ---
    let engine = base64::engine::general_purpose::STANDARD;
    loop {
        tokio::select! {
            chunk = audio_rx.recv() => {
                match chunk {
                    Some(bytes) if !bytes.is_empty() => {
                        let msg = serde_json::json!({
                            "message_type": "input_audio_chunk",
                            "audio_base_64": engine.encode(&bytes),
                            "commit": false,
                            "sample_rate": 16000
                        });
                        if ws_write.send(Message::Text(msg.to_string().into())).await.is_err() {
                            eprintln!("[error] WebSocket send failed");
                            break;
                        }
                    }
                    _ => break,
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n[info] Ctrl+C — sending commit...");
                let commit = serde_json::json!({
                    "message_type": "input_audio_chunk",
                    "audio_base_64": "",
                    "commit": true,
                    "sample_rate": 16000
                });
                let _ = ws_write.send(Message::Text(commit.to_string().into())).await;
                // Brief wait for final transcript
                tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
                let _ = ws_write.close().await;
                break;
            }
        }
    }

    drop(stream);
    recv_handle.abort();
    println!("[info] Done.");
    Ok(())
}

fn parse_arg(args: &[String], flag: &str) -> Result<String> {
    args.iter()
        .position(|a| a == flag)
        .and_then(|i| args.get(i + 1))
        .cloned()
        .context(format!("Missing {}", flag))
}

fn resample(input: &[f32], ratio: f64, state: &Arc<Mutex<(f64, f32)>>) -> Vec<f32> {
    let mut state = state.lock().unwrap();
    let mut output = Vec::with_capacity((input.len() as f64 * ratio) as usize + 1);
    for &sample in input {
        state.0 += ratio;
        while state.0 >= 1.0 {
            state.0 -= 1.0;
            let t = state.0 as f32;
            let interpolated = state.1 * t + sample * (1.0 - t);
            output.push(interpolated);
        }
        state.1 = sample;
    }
    output
}
