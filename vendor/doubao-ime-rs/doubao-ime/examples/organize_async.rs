//! 异步文字整理示例：`cargo run -p doubao-ime --example organize_async -- "文本"`
use doubao_ime::{Client, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let text: String = std::env::args().skip(1).collect::<Vec<_>>().join(" ");
    let text = if text.is_empty() {
        "嗯那个我想问一下明天几点开会".to_owned()
    } else {
        text
    };
    let client = Client::new()?;
    let out = client.organize(&text).await?;
    println!("原文: {text}");
    println!("整理: {}", out.content);
    Ok(())
}
