/// Wrap raw PCM (16-bit LE, 16 kHz, mono) in a minimal WAV container.
pub fn pcm_to_wav(pcm: &[u8]) -> Vec<u8> {
    let data_len = pcm.len() as u32;
    let sample_rate: u32 = 16000;
    let bits_per_sample: u16 = 16;
    let channels: u16 = 1;
    let byte_rate = sample_rate * u32::from(channels) * u32::from(bits_per_sample) / 8;
    let block_align = channels * bits_per_sample / 8;

    let mut wav = Vec::with_capacity(44 + pcm.len());
    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVE");
    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());
    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.extend_from_slice(pcm);
    wav
}

#[cfg(test)]
mod tests {
    use super::pcm_to_wav;

    #[test]
    fn header_has_riff_wave_magic() {
        let wav = pcm_to_wav(&[0xAA, 0xBB, 0xCC, 0xDD]);
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");
    }

    #[test]
    fn riff_chunk_size_is_36_plus_data_len() {
        let pcm = [0u8; 100];
        let wav = pcm_to_wav(&pcm);
        let riff_size = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_size, 36 + pcm.len() as u32);
    }

    #[test]
    fn fmt_chunk_fields_are_16khz_mono_16bit_pcm() {
        let wav = pcm_to_wav(&[0u8; 4]);
        let fmt_chunk_size = u32::from_le_bytes(wav[16..20].try_into().unwrap());
        let audio_format = u16::from_le_bytes(wav[20..22].try_into().unwrap());
        let channels = u16::from_le_bytes(wav[22..24].try_into().unwrap());
        let sample_rate = u32::from_le_bytes(wav[24..28].try_into().unwrap());
        let byte_rate = u32::from_le_bytes(wav[28..32].try_into().unwrap());
        let block_align = u16::from_le_bytes(wav[32..34].try_into().unwrap());
        let bits_per_sample = u16::from_le_bytes(wav[34..36].try_into().unwrap());

        assert_eq!(fmt_chunk_size, 16);
        assert_eq!(audio_format, 1); // PCM
        assert_eq!(channels, 1); // mono
        assert_eq!(sample_rate, 16000);
        assert_eq!(byte_rate, 16000 * 1 * 16 / 8); // 32000
        assert_eq!(block_align, 1 * 16 / 8); // 2
        assert_eq!(bits_per_sample, 16);
    }

    #[test]
    fn data_chunk_size_matches_pcm_len() {
        let pcm = [1u8, 2, 3, 4, 5, 6];
        let wav = pcm_to_wav(&pcm);
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len, pcm.len() as u32);
    }

    #[test]
    fn data_bytes_follow_header_unmodified() {
        let pcm = [1u8, 2, 3, 4, 5, 6];
        let wav = pcm_to_wav(&pcm);
        assert_eq!(&wav[44..], &pcm[..]);
        assert_eq!(wav.len(), 44 + pcm.len());
    }

    #[test]
    fn empty_pcm_produces_44_byte_header_only_wav() {
        let wav = pcm_to_wav(&[]);
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[0..4], b"RIFF");
        let riff_size = u32::from_le_bytes(wav[4..8].try_into().unwrap());
        assert_eq!(riff_size, 36);
        let data_len = u32::from_le_bytes(wav[40..44].try_into().unwrap());
        assert_eq!(data_len, 0);
    }
}
