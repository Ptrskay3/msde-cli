use std::process::Command;

#[derive(Debug)]
pub struct Compose {
    // pub stdin: std::io::Stdin,
    // pub stdout: std::io::Stdout,
    // pub stderr: std::io::Stderr,
    // pub cmd: std::process::Command,
}

#[derive(Default)]
pub struct ComposeOpts;

impl ComposeOpts {
    fn into_args<'a>(self) -> Vec<&'a str> {
        Vec::new()
    }
}

impl Compose {
    pub fn up(files: &[&str], opts: Option<ComposeOpts>) {
        let args: Vec<&str> = files.iter().flat_map(|file| ["-f", file]).collect();
        let opts = opts.unwrap_or_default().into_args();
        let cmd = Command::new("docker")
            .arg("compose")
            .args(args)
            .arg("up")
            .args(opts)
            .output();

        println!("{:#?}", cmd);
    }
}
