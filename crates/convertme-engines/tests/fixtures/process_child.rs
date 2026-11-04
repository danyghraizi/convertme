use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn main() {
    let mut args = std::env::args_os().skip(1);
    match args.next().as_deref().and_then(|v| v.to_str()) {
        Some("echo-file") => {
            let bytes = std::fs::read(args.next().expect("path")).unwrap();
            io::stdout().write_all(&bytes).unwrap();
        }
        Some("stderr-exit") => { eprintln!("boom"); std::process::exit(3); }
        Some("lines") => print!("a\nb\nc\n"),
        Some("sleep") => std::thread::sleep(Duration::from_secs(30)),
        Some("spawn-descendant") => {
            let marker = args.next().expect("marker path");
            let child = Command::new(std::env::current_exe().unwrap()).arg("descendant").arg(marker)
                .stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().unwrap();
            println!("{}", child.id());
            io::stdout().flush().unwrap();
            std::thread::sleep(Duration::from_secs(30));
        }
        Some("descendant") => {
            let marker = args.next().expect("marker path");
            std::thread::sleep(Duration::from_secs(3));
            std::fs::write(marker, b"alive").unwrap();
        }
        _ => std::process::exit(2),
    }
}
