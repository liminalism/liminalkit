//! Manual cross-process clipboard peer. Run only on an explicitly chosen private display.
use liminalkit_shell::{clipboard::ClipboardWorker, Clipboard};
use std::{
    io::{self, Write},
    time::Duration,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or("usage: clipboard_peer get|get-async | set|set-async TEXT HOLD_MS")?;
    match command.as_str() {
        "get" => {
            let clipboard = Clipboard::new();
            let text = clipboard.try_text()?.ok_or("clipboard contains no text")?;
            println!("{text}");
        }
        "set" => {
            let mut clipboard = Clipboard::new();
            let text = args.next().ok_or("set requires TEXT")?;
            let hold_ms = args.next().ok_or("set requires HOLD_MS")?.parse::<u64>()?;
            clipboard.try_set_text(text)?;
            println!("READY");
            io::stdout().flush()?;
            // Retain the actual X11/Wayland owner while another process reads.
            std::thread::sleep(Duration::from_millis(hold_ms));
        }
        "get-async" | "set-async" => {
            let (wake, notifications) = std::sync::mpsc::sync_channel(1);
            let worker = ClipboardWorker::new(move || {
                let _ = wake.try_send(());
            });
            let (task, hold_ms) = if command == "set-async" {
                let text = args.next().ok_or("set-async requires TEXT")?;
                let hold = args
                    .next()
                    .ok_or("set-async requires HOLD_MS")?
                    .parse::<u64>()?;
                (worker.copy(text)?, Some(hold))
            } else {
                (worker.paste()?, None)
            };
            // This peer stands in for an event-loop wake consumer, never a polling loop.
            notifications.recv_timeout(Duration::from_secs(10))?;
            let result = task
                .try_result()
                .ok_or("clipboard woke without a result")??;
            if let Some(hold_ms) = hold_ms {
                println!("READY");
                io::stdout().flush()?;
                std::thread::sleep(Duration::from_millis(hold_ms));
            } else {
                println!("{}", result.ok_or("clipboard contains no text")?);
            }
        }
        _ => return Err("usage: clipboard_peer get|get-async | set|set-async TEXT HOLD_MS".into()),
    }
    Ok(())
}
