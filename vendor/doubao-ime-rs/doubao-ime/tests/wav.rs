use doubao_ime::{read_wav_pcm, Error};

fn write_wav(path: &std::path::Path, rate: u32, channels: u16) {
    let spec = hound::WavSpec {
        channels,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for _ in 0..160 * channels as usize {
        w.write_sample(0i16).unwrap();
    }
    w.finalize().unwrap();
}

#[test]
fn reads_valid_and_rejects_wrong_rate() {
    let dir = tempdir();
    let ok = dir.join("ok.wav");
    write_wav(&ok, 16_000, 1);
    assert_eq!(read_wav_pcm(&ok).unwrap().len(), 320);

    let bad = dir.join("bad.wav");
    write_wav(&bad, 44_100, 1);
    assert!(matches!(read_wav_pcm(&bad), Err(Error::InvalidAudio(_))));
}

fn tempdir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("doubao-test-{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}
