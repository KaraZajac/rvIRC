//! File transfer via magic-wormhole: send and receive wrappers.

use crate::connection::IrcMessage;
use futures_util::io::AllowStdIo;
use std::sync::atomic::{AtomicU64, Ordering};
use magic_wormhole::transit::Abilities;
use magic_wormhole::{transfer, MailboxConnection, Wormhole};
use std::path::Path;
use tokio::sync::mpsc;

/// Send a file via magic wormhole. Sends the OFFER to the recipient as soon as
/// the wormhole code is available (before blocking on the transfer).
pub async fn send_file(
    path: &Path,
    nick: String,
    tx: mpsc::UnboundedSender<IrcMessage>,
) -> Result<(), String> {
    let file_name = path
        .file_name()
        .ok_or("no filename")?
        .to_string_lossy()
        .to_string();
    let metadata = std::fs::metadata(path).map_err(|e| format!("stat: {}", e))?;
    let file_size = metadata.len();

    let config = transfer::APP_CONFIG;
    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.clone(),
        text: "Creating wormhole connection...".to_string(),
    });

    let mailbox = MailboxConnection::create(config, 2)
        .await
        .map_err(|e| format!("wormhole mailbox: {}", e))?;
    let code = mailbox.code().to_string();

    let offer = format!("[:rvIRC:WORMHOLE:OFFER:{}:{}:{}]", code, file_name, file_size);
    let _ = tx.send(IrcMessage::SendPrivmsg {
        target: nick.clone(),
        text: offer,
    });
    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.clone(),
        text: format!("Wormhole code sent to {}. Waiting for accept...", nick),
    });
    let _ = tx.send(IrcMessage::Status(format!(
        "Waiting for {} to accept {}...",
        nick, file_name
    )));

    let wormhole = Wormhole::connect(mailbox)
        .await
        .map_err(|e| format!("wormhole connect: {}", e))?;

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.clone(),
        text: format!("Sending {} ({} bytes)...", file_name, file_size),
    });

    let f = std::fs::File::open(path).map_err(|e| format!("open file: {}", e))?;
    let buf = std::io::BufReader::new(f);
    let mut reader = AllowStdIo::new(buf);

    let last_pct = AtomicU64::new(0);
    let tx_progress = tx.clone();
    let nick_progress = nick.clone();
    let filename_progress = file_name.clone();
    let progress = move |bytes: u64, total: u64| {
        if total == 0 { return; }
        let pct = (bytes * 100) / total;
        let prev = last_pct.load(Ordering::Relaxed);
        if bytes == 0 || pct >= prev + 2 || bytes >= total {
            last_pct.store(pct.min(100), Ordering::Relaxed);
            let _ = tx_progress.send(IrcMessage::TransferProgress {
                nick: nick_progress.clone(),
                filename: filename_progress.clone(),
                bytes,
                total,
                is_send: true,
            });
        }
    };

    let result = {
        #[allow(deprecated)]
        transfer::send_file(
            wormhole,
            vec![],
            &mut reader,
            file_name.clone(),
            file_size,
            Abilities::ALL,
            |_info| {},
            progress,
            futures_util::future::pending(),
        )
        .await
    };

    let success = result.is_ok();
    let _ = tx.send(IrcMessage::TransferComplete {
        nick: nick.clone(),
        filename: file_name.clone(),
        is_send: true,
        success,
    });

    result.map_err(|e| format!("send: {}", e))?;

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.clone(),
        text: format!("File {} sent successfully.", file_name),
    });
    let _ = tx.send(IrcMessage::Status(format!(
        "File {} sent to {}.",
        file_name, nick
    )));

    Ok(())
}

/// Receive a file via magic wormhole. Saves to `save_path`.
pub async fn receive_file(
    code_str: &str,
    save_path: &Path,
    nick: &str,
    tx: &mpsc::UnboundedSender<IrcMessage>,
) -> Result<(), String> {
    let config = transfer::APP_CONFIG;
    let code: magic_wormhole::Code = code_str.to_string().into();

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.to_string(),
        text: "Connecting to wormhole relay...".to_string(),
    });

    let mailbox = MailboxConnection::connect(config, code, true)
        .await
        .map_err(|e| format!("wormhole mailbox: {}", e))?;

    let wormhole = Wormhole::connect(mailbox)
        .await
        .map_err(|e| format!("wormhole connect: {}", e))?;

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.to_string(),
        text: "Connected. Requesting file...".to_string(),
    });

    let req = transfer::request_file(
        wormhole,
        vec![],
        Abilities::ALL,
        futures_util::future::pending(),
    )
    .await
    .map_err(|e| format!("request: {}", e))?;

    let req = match req {
        Some(r) => r,
        None => return Err("transfer cancelled".to_string()),
    };

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.to_string(),
        text: format!("Receiving file to {}...", save_path.display()),
    });

    let filename_display = save_path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());

    let file = std::fs::File::create(save_path).map_err(|e| format!("create file: {}", e))?;
    let mut writer = AllowStdIo::new(std::io::BufWriter::new(file));

    let last_pct = AtomicU64::new(0);
    let tx_progress = tx.clone();
    let nick_progress = nick.to_string();
    let filename_progress = filename_display.clone();
    let progress = move |received: u64, total: u64| {
        if total == 0 { return; }
        let pct = (received * 100) / total;
        let prev = last_pct.load(Ordering::Relaxed);
        if received == 0 || pct >= prev + 2 || received >= total {
            last_pct.store(pct.min(100), Ordering::Relaxed);
            let _ = tx_progress.send(IrcMessage::TransferProgress {
                nick: nick_progress.clone(),
                filename: filename_progress.clone(),
                bytes: received,
                total,
                is_send: false,
            });
        }
    };

    let result = req
        .accept(
            |_info| {},
            progress,
            &mut writer,
            futures_util::future::pending(),
        )
        .await;

    let success = result.is_ok();
    let _ = tx.send(IrcMessage::TransferComplete {
        nick: nick.to_string(),
        filename: filename_display,
        is_send: false,
        success,
    });

    result.map_err(|e| format!("receive: {}", e))?;

    let _ = tx.send(IrcMessage::ChatLog {
        target: nick.to_string(),
        text: format!("File saved to {}", save_path.display()),
    });

    Ok(())
}
