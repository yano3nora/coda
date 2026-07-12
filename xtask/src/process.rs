// subprocess 実行の薄い wrapper。scripts/release.ts の run() 相当。
use std::process::Command;

/// run() の挙動フラグ。release.ts の RunOptions と同じ意味を持つ
#[derive(Default)]
pub struct RunOptions<'a> {
    // env に渡す追加の環境変数 (親プロセスの環境はそのまま継承した上で上書き・追加する)
    pub env: &'a [(&'a str, &'a str)],
    // quiet: command と stdout を表示しない。token など秘匿値を扱うコマンド用。
    // 失敗時の stderr だけは (秘匿値ではない前提で) 常に出す
    pub quiet: bool,
    // stream: stdout/stderr を端末へ直接流す (戻り値は空文字)。goreleaser 等の長時間コマンド用
    pub stream: bool,
}

/// command を実行し、成功時は stdout (stream=true の場合は空文字) を返す。
/// 非 0 exit は Err("Command failed: ...") にする
pub fn run(command: &str, args: &[&str], options: RunOptions) -> Result<String, String> {
    if !options.quiet {
        println!("$ {} {}", command, args.join(" "));
    }

    let mut cmd = Command::new(command);
    cmd.args(args);
    for (key, value) in options.env {
        cmd.env(key, value);
    }

    if options.stream {
        let status = cmd
            .status()
            .map_err(|err| format!("failed to run {command}: {err}"))?;
        if !status.success() {
            return Err(format!("Command failed: {command} {}", args.join(" ")));
        }
        return Ok(String::new());
    }

    let output = cmd
        .output()
        .map_err(|err| format!("failed to run {command}: {err}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();

    // quiet でも失敗時の stderr は出す。stdout は秘匿値の可能性があるため出さない
    if !options.quiet && !stdout.trim().is_empty() {
        println!("{}", stdout.trim_end());
    }
    if !stderr.trim().is_empty() && (!options.quiet || !output.status.success()) {
        eprintln!("{}", stderr.trim_end());
    }
    if !output.status.success() {
        return Err(format!("Command failed: {command} {}", args.join(" ")));
    }

    Ok(stdout)
}

// probe_args: version 表示の引数体系がツールごとに違う (zig は `zig version`)
pub fn command_exists(command: &str, probe_args: &[&str]) -> bool {
    run(
        command,
        probe_args,
        RunOptions {
            quiet: true,
            ..Default::default()
        },
    )
    .is_ok()
}
