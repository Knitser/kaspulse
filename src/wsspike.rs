use tungstenite::{connect, Message};
fn main() {
    let (mut s, _) = connect("wss://stream.bybit.com/v5/public/spot").expect("connect");
    s.write_message(Message::Text(r#"{"op":"subscribe","args":["tickers.KASUSDT","tickers.BTCUSDT"]}"#.into())).unwrap();
    let mut n = 0;
    let t0 = std::time::Instant::now();
    while n < 5 {
        match s.read_message() {
            Ok(Message::Text(t)) => {
                if let Ok(j) = serde_json::from_str::<serde_json::Value>(&t) {
                    if let (Some(sym), Some(px)) = (j["data"]["symbol"].as_str(), j["data"]["lastPrice"].as_str()) {
                        n += 1; println!("  tick#{n} @+{}ms: {sym}={px}", t0.elapsed().as_millis());
                    }
                }
            }
            Ok(Message::Ping(p)) => { let _ = s.write_message(Message::Pong(p)); }
            Ok(_) => {}
            Err(e) => { eprintln!("ws err: {e}"); break; }
        }
    }
    println!("rust tungstenite WS: OK");
}
