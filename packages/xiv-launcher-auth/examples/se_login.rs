use std::io::{self, Write};
use xiv_launcher_auth::crypto::make_computer_id;
use xiv_launcher_auth::se::SeAuth;

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

#[tokio::main]
async fn main() {
    env_logger::init();

    println!("=== Square Enix Auth Test ===\n");
    println!("Region: 1=JP, 2=NA, 3=EU");

    let region: i32 = prompt("Region").parse().unwrap_or(2);
    let is_free_trial = prompt("Free trial? (y/n)") == "y";
    let username = prompt("SQEX ID");
    let password = prompt_password("Password");
    let otp_str = prompt("OTP (or Enter to skip)");
    let otp = if otp_str.is_empty() {
        None
    } else {
        Some(otp_str)
    };
    let otp_ref: Option<&str> = otp.as_deref();

    let computer_id = make_computer_id(
        &sys_info::hostname().unwrap_or_default(),
        &whoami::username(),
        &format!("{:?}", std::env::consts::OS),
        num_cpus::get(),
    );
    println!("Computer ID: {}", computer_id);

    let auth = SeAuth::new().expect("Failed to create SE auth client");

    println!("\n--- Step 1: Get OAuth Top ---");
    match auth
        .login(
            &username,
            &password,
            otp_ref,
            region,
            is_free_trial,
            &computer_id,
            "en",
        )
        .await
    {
        Ok(result) => {
            println!("OK!");
            if let Some(ref oauth) = result.oauth_login {
                println!("Session ID: {}", oauth.session_id);
                println!("Region: {}", oauth.region);
                println!("Terms Accepted: {}", oauth.terms_accepted);
                println!("Playable: {}", oauth.playable);
                println!("Max Expansion: {}", oauth.max_expansion);

                println!("\n--- Step 2: Register Session ---");
                let boot_hash = "ffxivboot.exe/149504/5f2a70612aa58378eb347869e75adeb8f5581a1b";
                match auth
                    .register_session(oauth, "2024.11.01.0000.0000", boot_hash)
                    .await
                {
                    Ok(session_result) => {
                        println!("OK! State: {:?}", session_result.state);
                        if let Some(ref uid) = session_result.unique_id {
                            println!("Unique ID: {}", uid);
                        }
                    }
                    Err(e) => eprintln!("FAILED: {e}"),
                }
            }
        }
        Err(e) => eprintln!("FAILED: {e}"),
    }
}
