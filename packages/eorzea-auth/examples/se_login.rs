//! 国际服（Square Enix）OAuth 登录探针。
//!
//! 仅用于验证 `se` feature 的 login 流程，低优先级功能。

use std::io::{self, Write};

use eorzea_auth::se::SeAuth;

fn prompt(label: &str) -> String {
    print!("{label}: ");
    io::stdout().flush().unwrap();
    let mut buf = String::new();
    io::stdin().read_line(&mut buf).unwrap();
    buf.trim().to_string()
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();

    let username = prompt("SQEX account");
    let password = rpassword::prompt_password("Password: ").unwrap();
    let otp = prompt("One-time password (blank if unused)");
    let region = prompt("Region (1=JP, 2=NA, 3=EU)")
        .parse::<i32>()
        .unwrap_or(2);

    let computer_id = eorzea_auth::crypto::make_computer_id("eorzea", "eorzea", "Linux", 1);
    let auth = match SeAuth::new() {
        Ok(auth) => auth,
        Err(e) => {
            eprintln!("failed to create SeAuth: {e}");
            return;
        }
    };

    match auth
        .login(
            &username,
            &password,
            if otp.is_empty() { None } else { Some(&otp) },
            region,
            false,
            &computer_id,
            "en",
        )
        .await
    {
        Ok(result) => println!("login result: {result:?}"),
        Err(e) => eprintln!("login failed: {e}"),
    }
}
