use std::io::{BufRead, BufReader, Read};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

struct ServerChild(Child);

impl Drop for ServerChild {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

#[test]
fn local_client_connects_moves_receives_snapshots_and_disconnects() {
    let child = Command::new(env!("CARGO_BIN_EXE_net-poc"))
        .args([
            "server",
            "--bind",
            "127.0.0.1:0",
            "--sim-hz",
            "125",
            "--snapshot-hz",
            "60",
            "--exit-on-disconnect",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start local UDP server");
    let mut server = ServerChild(child);

    let stdout = server.0.stdout.take().expect("server stdout");
    let mut server_stdout = BufReader::new(stdout);
    let mut ready = String::new();
    server_stdout
        .read_line(&mut ready)
        .expect("read server readiness line");
    assert!(ready.starts_with("READY "), "server did not start: {ready}");
    assert!(
        ready.contains("sim_hz=125 snapshot_hz=60"),
        "server did not use independent configured rates: {ready}"
    );
    let address = ready
        .split_whitespace()
        .nth(1)
        .expect("server address in readiness line")
        .to_owned();

    let client = Command::new(env!("CARGO_BIN_EXE_net-poc"))
        .args([
            "mock-client",
            &address,
            "--duration-ms",
            "300",
            "--input-hz",
            "125",
            "--move-x",
            "1000",
        ])
        .output()
        .expect("run mock client");
    assert!(
        client.status.success(),
        "mock client failed:\n{}",
        String::from_utf8_lossy(&client.stderr)
    );
    let client_output = String::from_utf8_lossy(&client.stdout);
    assert!(client_output.contains("CLIENT connected client_id=1"));
    assert!(client_output.contains("disconnect=clean"));
    let snapshots = field_value(&client_output, "snapshots=");
    let final_tick = field_value(&client_output, "final_tick=");
    let final_x = field_value(&client_output, "final_x=");
    assert!(
        snapshots > 0,
        "client received no snapshots: {client_output}"
    );
    assert!(
        final_tick > 0,
        "server tick did not advance: {client_output}"
    );
    assert!(
        final_x > 0,
        "authoritative position did not advance: {client_output}"
    );

    wait_for_exit(&mut server.0);
    let status = server.0.wait().expect("wait for server exit");
    assert!(status.success(), "server did not exit cleanly: {status}");
    let mut server_output = String::new();
    server_stdout
        .read_to_string(&mut server_output)
        .expect("read server output");
    assert!(
        server_output.contains("disconnect=clean"),
        "server did not record clean disconnect: {server_output}"
    );
    assert!(
        server_output.contains("SERVER stopped"),
        "server did not stop after disconnect: {server_output}"
    );
}

fn field_value(output: &str, field: &str) -> u32 {
    output
        .split_whitespace()
        .find_map(|part| part.strip_prefix(field))
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| panic!("missing {field} in output: {output}"))
}

fn wait_for_exit(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if child.try_wait().expect("poll server").is_some() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "server did not stop after disconnect"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
