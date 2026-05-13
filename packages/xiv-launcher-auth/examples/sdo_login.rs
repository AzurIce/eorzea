use std::io::{self, Write};
use xiv_launcher_auth::sdo::SdoAuth;

fn prompt(label: &str) -> String {
    print!("{}: ", label);
    io::stdout().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    buf.trim().to_string()
}

fn prompt_password(label: &str) -> String {
    print!("{}: ", label);
    io::stdout().flush().unwrap();
    rpassword::read_password().unwrap()
}

async fn do_sso_flow(auth: &SdoAuth, ctx: &xiv_launcher_auth::sdo::SdoContext, tgt: &str, snda_id: Option<&str>) -> Option<String> {
    println!("\n--- getPromotionInfo ---");
    if let Err(e) = auth.get_promotion_info(tgt).await {
        eprintln!("FAILED: {e}");
        return None;
    }
    println!("OK");

    println!("\n--- ssoLogin ---");
    match auth.sso_login(ctx, tgt).await {
        Ok(ticket) => {
            println!("OK: ticket={}", ticket);
            let _ = snda_id;
            Some(ticket)
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            None
        }
    }
}

#[tokio::main]
async fn main() {
    env_logger::init();

    println!("=== SDO Auth Test ===\n");
    println!("Choose login method:");
    println!("  1. Password (static)");
    println!("  2. Slide (push message)");
    println!("  3. QR Code");
    println!("  4. Auto-login (session key)");
    println!("  5. Server list only");
    print!("> ");
    io::stdout().flush().unwrap();

    let mut choice = String::new();
    io::stdin().read_line(&mut choice).unwrap();
    let choice = choice.trim();

    if choice == "5" {
        match SdoAuth::fetch_server_list().await {
            Ok(areas) => {
                println!("\n=== Server List ===");
                for area in &areas {
                    println!(
                        "  {} (ID: {}): lobby={} gm={} patch={}",
                        area.area_name,
                        area.area_id,
                        area.area_lobby,
                        area.area_gm,
                        area.area_patch
                    );
                }
                println!("\n{} areas loaded.", areas.len());
            }
            Err(e) => eprintln!("Error: {e}"),
        }
        return;
    }

    let auth = SdoAuth::new().expect("Failed to create SdoAuth client");

    println!("Device ID: {}", xiv_launcher_auth::sdo_device::get_device_id());
    println!("MAC ID:    {}", xiv_launcher_auth::sdo_device::get_mac_address_hash());

    println!("\n--- getGuid ---");
    let ctx = match auth.get_context().await {
        Ok(ctx) => {
            println!("OK: guid={}, dynamic_key={:?}", ctx.guid, ctx.dynamic_key);
            ctx
        }
        Err(e) => {
            eprintln!("FAILED: {e}");
            return;
        }
    };

    let mut ticket: Option<String> = None;
    let mut snda_id: Option<String> = None;

    match choice {
        "1" => {
            let account = prompt("Account");
            let password = prompt_password("Password");
            println!("\n--- staticLogin ---");
            match auth.static_login(&ctx, &account, &password).await {
                Ok(result) => {
                    println!("OK: return_code={}", result.return_code);
                    if let Some(ref fail) = result.data.fail_reason {
                        if !fail.is_empty() { println!("fail_reason={}", fail); }
                    }
                    if let Some(ref sk) = result.data.auto_login_session_key {
                        println!("auto_login_session_key={}", sk);
                        println!("\n--- Save this key for auto-login next time ---");
                    }
                    println!("  snda_id={:?}", result.data.snda_id);
                    println!("  tgt={:?}", result.data.tgt);
                    println!("  input_user_id={:?}", result.data.input_user_id);
                    snda_id = result.data.snda_id.clone();
                    if let Some(ref tgt) = result.data.tgt {
                        ticket = do_sso_flow(&auth, &ctx, tgt, snda_id.as_deref()).await;
                    } else {
                        println!("\n[WARN] No tgt in static_login response. Full data: {:?}", result.data);
                    }
                }
                Err(e) => eprintln!("FAILED: {e}"),
            }
        }
        "2" => {
            let account = prompt("Account");
            println!("\n--- sendPushMessage ---");
            match auth.slide_login_request(&ctx, &account).await {
                Ok(data) => {
                    println!("OK: push_msg_session_key={:?}", data.push_msg_session_key);
                    if let Some(ref key) = data.push_msg_session_key {
                        println!("\nWaiting for phone confirmation...");
                        loop {
                            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
                            match auth.slide_login_poll(&ctx, key).await {
                                Ok(xiv_launcher_auth::sdo::PollResult::Success(data)) => {
                                    println!("\nOK: Login confirmed!");
                                    snda_id = data.snda_id.clone();
                                    if let Some(ref sk) = data.auto_login_session_key {
                                        println!("auto_login_session_key={}", sk);
                                    }
                                    if let Some(ref tgt) = data.tgt {
                                        ticket = do_sso_flow(&auth, &ctx, tgt, snda_id.as_deref()).await;
                                    }
                                    break;
                                }
                                Ok(xiv_launcher_auth::sdo::PollResult::Pending) => {
                                    print!(".");
                                    io::stdout().flush().unwrap();
                                }
                                Err(e) => {
                                    eprintln!("\nFAILED: {e}");
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => eprintln!("FAILED: {e}"),
            }
        }
        "3" => {
            println!("\n--- getCodeKey (QR Code) ---");
            match auth.qr_code_request(&ctx).await {
                Ok(result) => {
                    println!("code_key={}", result.code_key);
                    let qr_path = "/tmp/xiv_qr.png";
                    std::fs::write(qr_path, &result.image_data).unwrap();
                    println!("QR image saved to {} ({} bytes)", qr_path, result.image_data.len());

                    println!("\nPolling for QR scan...");
                    loop {
                        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                        match auth.qr_code_poll(&ctx, &result.code_key, 30).await {
                            Ok(xiv_launcher_auth::sdo::PollResult::Success(data)) => {
                                println!("\nOK: QR scanned and confirmed!");
                                println!("  snda_id={:?}", data.snda_id);
                                println!("  tgt={:?}", data.tgt);
                                println!("  input_user_id={:?}", data.input_user_id);
                                println!("  auto_login_session_key={:?}", data.auto_login_session_key);
                                snda_id = data.snda_id.clone();
                                if let Some(ref tgt) = data.tgt {
                                    ticket = do_sso_flow(&auth, &ctx, tgt, snda_id.as_deref()).await;
                                } else {
                                    println!("\n[WARN] No tgt in QR login response. Full data: {:?}", data);
                                }
                                break;
                            }
                            Ok(xiv_launcher_auth::sdo::PollResult::Pending) => {
                                print!(".");
                                io::stdout().flush().unwrap();
                            }
                            Err(e) => {
                                eprintln!("\nFAILED: {e}");
                                break;
                            }
                        }
                    }
                }
                Err(e) => eprintln!("FAILED: {e}"),
            }
        }
        "4" => {
            let session_key = prompt("Auto-login session key");
            println!("\n--- autoLogin ---");
            match auth.auto_login(&ctx, &session_key).await {
                Ok(result) => {
                    println!("OK: return_code={}", result.return_code);
                    snda_id = result.data.snda_id.clone();
                    if let Some(ref tgt) = result.data.tgt {
                        ticket = do_sso_flow(&auth, &ctx, tgt, snda_id.as_deref()).await;
                    }
                }
                Err(e) => eprintln!("FAILED: {e}"),
            }
        }
        _ => println!("Invalid choice"),
    }
}