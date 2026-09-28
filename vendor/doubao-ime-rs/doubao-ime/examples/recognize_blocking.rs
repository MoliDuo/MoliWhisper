//! 同步识别示例：`cargo run -p doubao-ime --example recognize_blocking -- audio.wav`
use doubao_ime::blocking::Client;
use doubao_ime::{AsrOptions, Result};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("用法: recognize_blocking <wav>");
    let client = Client::new()?;
    let text = client.recognize_file(&path, AsrOptions::default())?;
    println!("{text}");
    Ok(())
}
