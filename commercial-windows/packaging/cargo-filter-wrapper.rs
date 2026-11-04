use std::{
    env,
    process::{exit, Command},
};

fn main() {
    let args: Vec<_> = env::args_os().skip(1).collect();
    let cargo = env::var_os("CONVERTME_REAL_CARGO").expect("CONVERTME_REAL_CARGO is not set");
    let mut command = Command::new(cargo);
    if args.first().is_some_and(|arg| arg == "metadata") {
        command
            .args(&args)
            .arg("--filter-platform")
            .arg("x86_64-pc-windows-msvc");
    } else {
        command.args(&args);
    }
    let status = command.status().expect("failed to run cargo");
    exit(status.code().unwrap_or(1));
}
