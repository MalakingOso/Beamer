use std::io::Cursor;
use rodio::Source;

fn main() {
    let start_data: &[u8] = include_bytes!("../assets/startsound.mp3");
    let end_data: &[u8] = include_bytes!("../assets/endsound.mp3");

    println!("Start sound: {} bytes, 48ms, max amp 49%", start_data.len());
    println!("End sound: {} bytes, 2037ms, max amp 12%", end_data.len());

    // Test: play with cpal input active (simulates Dioxus app)
    println!("\n=== Simulating app: cpal mic + rodio playback ===");
    println!("Opening cpal input stream...");
    let host = cpal::default_host();
    use cpal::traits::{HostTrait, DeviceTrait, StreamTrait};
    let device = host.default_input_device().expect("No input device");
    let config = device.default_input_config().expect("No input config");
    let stream = device.build_input_stream(
        &config.into(),
        |_data: &[f32], _info: &cpal::InputCallbackInfo| {},
        |err| eprintln!("cpal error: {}", err),
        None,
    ).expect("Failed to build input stream");
    stream.play().expect("Failed to start input stream");
    println!("Mic capturing (like in the app)...");

    println!("\nPlaying END sound while mic is active...");
    let h = play_threaded(end_data);
    h.join().unwrap();

    println!("\nPlaying START sound while mic is active...");
    let h = play_threaded(start_data);
    h.join().unwrap();

    drop(stream);
    println!("\nDone!");
}

fn play_threaded(data: &'static [u8]) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let (_stream, handle) = match rodio::OutputStream::try_default() {
            Ok(v) => { println!("  Output device opened"); v }
            Err(e) => { println!("  OUTPUT DEVICE FAILED: {}", e); return; }
        };
        let sink = match rodio::Sink::try_new(&handle) {
            Ok(v) => v,
            Err(e) => { println!("  SINK FAILED: {}", e); return; }
        };
        let cursor = Cursor::new(data);
        let source = match rodio::Decoder::new(cursor) {
            Ok(v) => v,
            Err(e) => { println!("  DECODE FAILED: {}", e); return; }
        };
        sink.append(source);
        println!("  Playing...");
        sink.sleep_until_end();
        std::thread::sleep(std::time::Duration::from_millis(100));
        println!("  Done");
    })
}
