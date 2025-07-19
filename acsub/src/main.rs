use std::{env::{args, current_dir}, fs::{read_to_string, File}, io::{Read, Stderr, Write}, process::{exit, Command, Stdio}};
use dirs::home_dir;

use serde::Deserialize;
use toml::{from_str, Value};

#[derive(Deserialize)]
struct Config {
    url: String,
}

enum SubmitError {
    SampleFailed,
    CommandExecuteFailed,
}

fn submit_url(problem_id: &String) -> String {
    // $(pwd) の ac_config.tomlを読む
    // 存在しない場合はエラー
    let path = current_dir().unwrap().join("ac_config.toml");
    if !path.exists() {
        eprintln!("ac_config.toml not found.");
        exit(1);
    }
    
    let src = read_to_string(&path).expect("failed to read content.");
    let cfg: Config = from_str(&src).expect("failed to parse.");
    // URLを生成
    let place_holder = "{problem_id}";

    cfg.url.replace(place_holder, problem_id)
}

fn utf8_to_utf16le_bytes(src: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + src.len() * 2 + 2);
    v.extend_from_slice(&[0xFF, 0xFE]); // BOM
    for u in src.encode_utf16() {
        v.push((u & 0x00FF) as u8);
        v.push((u >> 8) as u8);
    }
    v.extend_from_slice(&[0x00, 0x00]); // NUL 2Byte
    v
}

fn submit(lang: &String, id: &String, url: &String, is_check: bool) -> Result<(), SubmitError> {
    let _output = Command::new("rm")
        .args(["-rf", "test"])
        .status();

    let output = Command::new("oj")
        .args(["d", url])
        .stdout(Stdio::inherit())
        .status();
    if output.is_err() {
        return Err(SubmitError::CommandExecuteFailed);
    }


    if lang == &"rs".to_string() {
        if is_check {
            // テスト実行
            let execute_command = format!("cargo run --features local --bin {}",id);
            let output = Command::new("oj")
                .args(["t", "-c"])
                .arg(execute_command)
                .stdout(Stdio::inherit())
                .status();
            if output.is_err() {
                return Err(SubmitError::CommandExecuteFailed);
            }
            if output.unwrap().code().unwrap() > 0 {
                return Err(SubmitError::SampleFailed);
            }
        }

        // ファイルマージ
        // let output = Command::new("uv")
        //     .args(["run", "python3"])
        //     .arg("../../util/file_merger.py")
        //     .arg(id)
        //     .stdout(Stdio::inherit())
        //     .status();
        // if output.is_err() {
        //     return Err(SubmitError::CommandExecuteFailed);
        // }

        let lib_root = home_dir().expect("Could not determine home directory")
            .join("repos")
            .join("adry_library")
            .join("adry_library")
            .join("src");

        let target   = format!("src/bin/{id}.rs");

        let bundler_out = Command::new("bundler")
            .arg(&lib_root)
            .arg(&target)
            .output()
            .map_err(|_| SubmitError::CommandExecuteFailed)?;

        if !bundler_out.status.success() {
            eprintln!("bundler failed");
            return Err(SubmitError::CommandExecuteFailed);
        }

        let bundled_src = String::from_utf8_lossy(&bundler_out.stdout);

        // 3) submit.rs へ保存
        let mut file = File::create("submit.rs").map_err(|_| SubmitError::CommandExecuteFailed)?;
        file.write_all(bundled_src.as_bytes())
            .map_err(|_| SubmitError::CommandExecuteFailed)?;

        // 4) クリップボードへコピー（UTF-16LE）
        let utf16_bytes = utf8_to_utf16le_bytes(&bundled_src);
        let mut child = Command::new("clip.exe")
            .stdin(Stdio::piped())
            .spawn()
            .map_err(|_| SubmitError::CommandExecuteFailed)?;
        {
            let stdin = child.stdin.as_mut().unwrap();
            stdin.write_all(&utf16_bytes).unwrap();
        }
        child.wait().unwrap();
    } else if lang == &"py".to_string() {
        todo!()
    } else if lang == &"cpp".to_string() {
        todo!()
    } else {
        eprintln!("language {} is not supported.", lang);
        exit(1);
    }

    Ok(())
}

fn main() {
    let args = args().collect::<Vec<String>>();
    if args.len() < 3 {
        eprintln!("Usage: acsub <language> <problem id>");
        eprintln!("options:");
        eprintln!("  --with-no-test: sampleチェック無しでコピー");
        exit(1);
    }

    let language = args[1].clone();
    let problem_id = args[2].clone();
    let v = args[3..].iter().cloned().collect::<Vec<String>>();
    let is_check = !v.contains(&"--with-no-test".to_string());

    let url = submit_url(&problem_id);
    if let Err(er) = submit(&language, &problem_id, &url, is_check) {
        match er {
            SubmitError::CommandExecuteFailed => {
                eprintln!("Something Wrong.")
            },
            SubmitError::SampleFailed => {
                eprintln!("Wrong Answer, or Runtime Error occured.")
            }
        }
        exit(1);
    }

    println!("All Tests passed🎉 Code was copied to clipboard!");
}