use std::{error::Error, time::Duration, path::{Path, PathBuf}};
use shell_escape::unix::escape;
use openssh::{Session, SessionBuilder, Stdio, KnownHosts};
use openssh_sftp_client::{Sftp, file::TokioCompatFile};
use clap::Parser;
use tokio::{io::{copy, AsyncRead, BufReader, AsyncBufReadExt}, time::{timeout, interval}, net::TcpStream};
use regex::Regex;

#[derive(Debug, Parser)]
struct Args {
    #[clap(long)]
    user: String,

    #[clap(long)]
    host: String,

    #[clap(long, default_value = "22")]
    port: u16,

    #[clap(long, default_value = "~/.ssh/id_rsa")]
    keyfile: PathBuf,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    let host = &args.host;
    let port = args.port;

    wait_for_ssh_connectable(host, port).await?;

    let session = SessionBuilder::default()
        .user(args.user)
        .port(args.port)
        .keyfile(args.keyfile)
        .connect_timeout(Duration::from_secs(10))
        .known_hosts_check(KnownHosts::Add)
        .server_alive_interval(Duration::from_secs(60))
        .connect_mux(args.host)
        .await?;

    command_list(&session).await?;

    put_data_file(&session, Path::new("test.txt"), &b"hey"[..]).await?;

    Ok(())
}

async fn wait_for_ssh_connectable(host: &str, port: u16) -> Result<(), Box<dyn Error>> {
    
    // lightweight ssh connection check than connect

    let mut interval = interval(Duration::from_secs(10));

    loop {
        match timeout(Duration::from_secs(5), TcpStream::connect((host, port))).await {
            Err(_) => {
                eprintln!("waiting for ssh: timeout");
                interval.tick().await;
            }
            Ok(Err(e)) => {
                eprintln!("waiting for ssh: {}", e);
                interval.tick().await;
            }
            Ok(Ok(_)) => {
                return Ok(());
            }
        }
    }
}

async fn command_list(session: &Session) -> Result<(), Box<dyn Error>> {

    // example for showing executing command and parsing output

    let mut ps_process = session.raw_command(vec!["ps", "auwx"].into_iter().map(|s| escape(s.into()).to_string()).collect::<Vec<_>>().join(" "))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .await?;

    let stdout = ps_process.stdout().take().expect("should be piped one");
    let mut line_stream = BufReader::new(stdout).lines();

    let first_line = line_stream.next_line().await?;
    let Some(first_line) = first_line else {
        return Err("output no line".into());
    };
    let headers = first_line.split_whitespace().collect::<Vec<_>>();
    assert_eq!(headers.len(), 11);
    assert_eq!(headers.last().expect("last"), &"COMMAND");
    println!("{:?}", headers);

    let regex = Regex::new(r"^(?:[^\s]+\s+){10}(.*)$").expect("hardcoded regex");
    while let Some(record) = line_stream.next_line().await? {
        let captures = regex.captures(&record).expect("regex should match");
        let command = captures.get(1).expect("should have capture").as_str();
        println!("{}", command);
    }

    ps_process.wait().await?;

    Ok(())
}

async fn put_data_file(session: &Session, remote_path: &Path, mut data: impl AsyncRead + Unpin) -> Result<(), Box<dyn Error>> {

    // example for putting data to remote file
    // AsyncRead accepts almost types of input stream, or fixed data

    let mut sftp_process = session
        .subsystem("sftp")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .await?;

    let sftp = Sftp::new(
        sftp_process.stdin().take().expect("should be piped"),
        sftp_process.stdout().take().expect("should be piped"),
        Default::default(),
    ).await?;

    let remote_file = sftp.create(remote_path).await?;
    let mut remote_file = Box::pin(TokioCompatFile::new(remote_file)); // tokio copy requires Unpin

    copy(&mut data, &mut remote_file).await?;

    Ok(())
}

