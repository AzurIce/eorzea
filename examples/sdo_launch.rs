use std::io::{self, Write};
use std::path::PathBuf;
use tracing::{error, info};
use xiv_launcher_auth::sdo::SdoAuth;
use xiv_launcher_rs_lib::launcher::{Launcher, LoginMethod};

fn prompt(label: &str) -> String {
    print!("{}: ", label);
    io::stdout().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    buf.trim().to_string()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    info!("=== XIV Launcher - SDO Login & Launch ===\n");

    // 1. 选择大区
    let areas = SdoAuth::fetch_server_list()
        .await
        .expect("Failed to fetch server list");
    println!("Available areas:");
    for (i, a) in areas.iter().enumerate() {
        println!("  {}: {}", i, a.area_name);
    }
    let idx: usize = prompt("Select area index (0-3)").parse().unwrap_or(0);
    let area = areas[idx].clone();

    // 2. 选择登录方式
    println!("\nChoose login method:");
    println!("  1. Password (static)");
    println!("  2. QR Code");
    println!("  3. Auto-login (session key)");
    print!("> ");
    io::stdout().flush().unwrap();

    let mut choice = String::new();
    io::stdin().read_line(&mut choice).unwrap();
    let choice = choice.trim();

    // 3. 创建 Launcher
    let launcher = Launcher::new().expect("Failed to create launcher");
    info!("Device ID: {}", launcher.device_id());
    info!("MAC ID: {}", launcher.mac_id());

    // 4. 登录
    let token = match choice {
        "1" => {
            let account = prompt("Account");
            let password = rpassword::read_password().unwrap();
            launcher
                .login(LoginMethod::Password {
                    account: &account,
                    password: &password,
                })
                .await
        }
        "2" => {
            // QR 码登录：先请求二维码，展示给用户，然后轮询
            let ctx = launcher.get_context().await.expect("get_context failed");
            let (code_key, png_bytes) = launcher
                .request_qr_code(&ctx)
                .await
                .expect("qr_code_request failed");

            let qr_path = "/tmp/xiv_qr.png";
            std::fs::write(qr_path, &png_bytes).unwrap();
            println!("\nQR image saved to {} ({} bytes)", qr_path, png_bytes.len());
            println!("Please scan with Daoyu APP...\n");

            // 轮询扫码结果
            let login_data = loop {
                tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
                match launcher.poll_qr_code(&ctx, &code_key).await {
                    Ok(Some(data)) => {
                        println!("QR scanned and confirmed!");
                        break data;
                    }
                    Ok(None) => {
                        print!(".");
                        io::stdout().flush().unwrap();
                    }
                    Err(e) => {
                        error!("QR poll failed: {}", e);
                        panic!("QR login failed");
                    }
                }
            };

            // 手动走完剩余流程（因为 login() 方法内部会处理，但 QR 需要外部轮询）
            let snda_id = login_data
                .snda_id
                .clone()
                .expect("no snda_id in qr response");
            let tgt = login_data
                .tgt
                .clone()
                .expect("no tgt in qr response");

            // 激活 + sso_login
            launcher
                .auth
                .get_promotion_info(&tgt)
                .await
                .expect("get_promotion_info failed");
            let ticket = launcher
                .auth
                .sso_login(&ctx, &tgt)
                .await
                .expect("sso_login failed");

            Ok(xiv_launcher_rs_lib::launcher::LaunchToken {
                ticket,
                snda_id,
                auto_login_session_key: login_data.auto_login_session_key,
            })
        }
        "3" => {
            let session_key = prompt("Auto-login session key");
            launcher
                .login(LoginMethod::AutoLogin {
                    session_key: &session_key,
                })
                .await
        }
        _ => {
            error!("Invalid choice");
            return;
        }
    };

    let token = match token {
        Ok(t) => {
            info!("Login successful!");
            if let Some(ref sk) = t.auto_login_session_key {
                println!("\nAuto-login session key (save for next time): {}", sk);
            }
            t
        }
        Err(e) => {
            error!("Login failed: {}", e);
            return;
        }
    };

    // 5. 启动游戏
    let game_path = PathBuf::from("/Volumes/Files/_ffxiv/XIVLauncherGamePath/game/ffxiv_dx11.exe");
    println!("\nLaunching game...");
    match launcher
        .launch(&token, area, areas, &game_path)
        .await
    {
        Ok(result) => {
            info!("Game launched! PID: {}", result.child.id());
            println!("Command: {}", result.command);
        }
        Err(e) => {
            error!("Launch failed: {}", e);
        }
    }
}
