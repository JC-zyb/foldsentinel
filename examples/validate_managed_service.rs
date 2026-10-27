//! 已安装服务的显式实际IO验收；不安装服务，不删除恢复资料或测试内容。
use anyhow::{Context, Result, bail, ensure};
use foldsentinel::{ProtectionMode, StateStore, service};
use rand::RngCore;
use serde_json::{Value, json};
use std::{
    fs,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

fn call(command: service::Command) -> Result<Value> {
    let response = service::request(&service::Request::new(command))?;
    ensure!(response.transport.host == "service", "必须连接正式服务");
    ensure!(response.transport.healthy, "服务扫描存在待处理错误");
    response.into_result()
}

fn denied(path: &Path) -> Result<()> {
    let error = fs::write(path, b"must-not-be-written")
        .err()
        .context("受保护文件必须拒绝写入")?;
    ensure!(
        error.kind() == std::io::ErrorKind::PermissionDenied,
        "预期权限拒绝，实际为{error}"
    );
    Ok(())
}

fn wait_for_relock(path: &Path) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        match fs::OpenOptions::new().write(true).open(path) {
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => return Ok(()),
            Err(error) => return Err(error.into()),
            Ok(_) if Instant::now() < deadline => thread::sleep(Duration::from_millis(200)),
            Ok(_) => bail!("等待自动重锁超时"),
        }
    }
}

fn run(fixture: &Path, checks: &mut Vec<Value>) -> Result<()> {
    ensure!(
        call(service::Command::List)? == json!([]),
        "验收要求当前账户没有既有服务项目"
    );
    let target = fixture.join("SYSTEM服务验收");
    fs::create_dir(&target)?;
    let file = target.join("原始资料.txt");
    fs::write(&file, "original-content")?;
    let mut secret = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut secret);
    let password = Zeroizing::new(
        secret
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>(),
    );
    let mut active_id: Option<String> = None;
    let result = (|| -> Result<()> {
        let enrolled = call(service::Command::Enroll {
            path: fs::canonicalize(&target)?,
            mode: ProtectionMode::ReadonlyAcl,
            note: "仅自有夹具".to_owned(),
            password: Some(password.to_string()),
        })?;
        let id = enrolled["id"].as_str().context("登记缺少ID")?.to_owned();
        active_id = Some(id.clone());
        ensure!(fs::read_to_string(&file)? == "original-content");
        denied(&file)?;
        denied(&target.join("不得新建.txt"))?;
        checks.push(json!({"name":"SYSTEM登记权限只读，可读且拒写/新建","passed":true}));
        let before = call(service::Command::List)?;
        ensure!(
            call(service::Command::Unlock {
                id: id.clone(),
                password: "intentionally-wrong".to_owned(),
                duration_seconds: 300
            })
            .is_err()
        );
        ensure!(
            call(service::Command::List)? == before,
            "错误密码不能改变服务状态"
        );
        denied(&file)?;
        checks.push(json!({"name":"错误密码不修改状态或权限","passed":true}));
        let mut local = StateStore::load_from(fixture.join("local-state.json"))?;
        ensure!(
            local.protect(&target, ProtectionMode::Hidden).is_err(),
            "服务先登记后本地保护必须拒绝"
        );
        ensure!(local.entries().count() == 0);
        checks.push(json!({"name":"服务范围拒绝本地叠加","passed":true}));
        let request: service::Request = serde_json::from_value(
            json!({"version":service::PROTOCOL_VERSION,"command":{"action":"unlock","id":id,"password":password.as_str()}}),
        )?;
        let started = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        service::request(&request)?.into_result()?;
        let view = call(service::Command::List)?;
        let deadline = view[0]["unlocked_until"]
            .as_u64()
            .context("缺少临时访问截止")?;
        ensure!((started + 300..=started + 305).contains(&deadline));
        fs::write(&file, "authenticated-content")?;
        let child = target.join("解锁期间新增.txt");
        fs::write(&child, "new-child-content")?;
        call(service::Command::Relock { id: id.clone() })?;
        denied(&file)?;
        denied(&child)?;
        checks.push(json!({"name":"默认五分钟，正确验证可写，立即重锁并登记新增项","passed":true}));
        // 移除本次夹具单个文件的显式ACL，实际后台服务应修复漂移。
        let reset = std::process::Command::new("icacls.exe")
            .arg(&file)
            .arg("/reset")
            .output()?;
        ensure!(reset.status.success(), "隔离夹具ACL漂移设置失败");
        wait_for_relock(&file)?;
        checks.push(json!({"name":"GUI关闭时SYSTEM后台修复ACL漂移","passed":true}));
        call(service::Command::Unlock {
            id: id.clone(),
            password: password.to_string(),
            duration_seconds: 30,
        })?;
        fs::write(&file, "before-expiry")?;
        println!("等待30秒真实截止重锁");
        thread::sleep(Duration::from_secs(31));
        wait_for_relock(&file)?;
        ensure!(call(service::Command::List)?[0]["unlocked_until"].is_null());
        checks.push(json!({"name":"真实时钟截止后服务自动重锁","passed":true}));
        for phase in ["restart", "crash"] {
            call(service::Command::Unlock {
                id: id.clone(),
                password: password.to_string(),
                duration_seconds: 300,
            })?;
            fs::write(&file, format!("before-{phase}"))?;
            fs::write(
                fixture.join("phase.json"),
                serde_json::to_vec_pretty(&json!({"phase":phase,"ready":true}))?,
            )?;
            println!("等待已授权SCM操作：{phase}");
            let until = Instant::now() + Duration::from_secs(180);
            let marker = fixture.join(format!("{phase}-complete"));
            while !marker.exists() {
                ensure!(Instant::now() < until, "等待SCM验收操作超时");
                thread::sleep(Duration::from_millis(250));
            }
            wait_for_relock(&file)?;
            ensure!(call(service::Command::List)?[0]["unlocked_until"].is_null());
            checks.push(
                json!({"name":format!("SCM {phase} 后恢复未完成临时访问并重锁"),"passed":true}),
            );
        }
        let serialized = serde_json::to_string(&call(service::Command::List)?)?;
        ensure!(
            !serialized.contains("原始资料.txt")
                && !serialized.contains("D:P")
                && !serialized.contains(password.as_str())
                && !serialized.contains(&fixture.to_string_lossy().to_string())
        );
        checks.push(json!({"name":"响应不含父路径、子文件名、ACL或密码","passed":true}));
        call(service::Command::Remove {
            id: id.clone(),
            password: Some(password.to_string()),
        })?;
        active_id = None;
        ensure!(fs::read_to_string(&file)? == "before-crash");
        ensure!(fs::read_to_string(&child)? == "new-child-content");
        fs::write(&file, "restored-write-access")?;
        ensure!(call(service::Command::List)? == json!([]));
        checks.push(json!({"name":"解除保留内容并恢复可写，当前账户无服务残留","passed":true}));
        // 本地先登记后再服务登记的官方入口检查。
        local.protect(&target, ProtectionMode::Hidden)?;
        ensure!(
            local
                .ensure_service_registration_available(&target)
                .is_err()
        );
        local.unprotect(&target)?;
        checks.push(json!({"name":"本地先登记拒绝服务叠加，解除成功","passed":true}));
        let hidden = call(service::Command::Enroll {
            path: fs::canonicalize(&target)?,
            mode: ProtectionMode::HiddenLocked,
            note: String::new(),
            password: Some(password.to_string()),
        })?;
        let hidden_id = hidden["id"].as_str().context("登记缺少ID")?.to_owned();
        active_id = Some(hidden_id.clone());
        ensure!(
            fs::read_to_string(&file).is_err(),
            "隐藏并锁定应拒绝内容读取"
        );
        call(service::Command::Remove {
            id: hidden_id,
            password: Some(password.to_string()),
        })?;
        active_id = None;
        ensure!(fs::read_to_string(&file)? == "restored-write-access");
        ensure!(call(service::Command::List)? == json!([]));
        checks.push(json!({"name":"SYSTEM隐藏并锁定拒读，解除后内容不变","passed":true}));
        Ok(())
    })();
    if let Some(id) = active_id {
        call(service::Command::Remove {
            id,
            password: Some(password.to_string()),
        })
        .context("验收失败后的夹具恢复也失败，请保留服务记录")?;
    }
    result
}

fn main() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    ensure!(
        args.next().as_deref() == Some(std::ffi::OsStr::new("--allow-installed-service")),
        "须显式授权已安装服务验收"
    );
    let fixture = PathBuf::from(args.next().context("需要新夹具目录")?);
    ensure!(
        fixture.is_absolute() && !fixture.exists() && args.next().is_none(),
        "必须指定尚不存在的绝对夹具目录"
    );
    fs::create_dir(&fixture)?;
    let mut checks = Vec::new();
    let result = run(&fixture, &mut checks);
    let report = json!({"passed":result.is_ok(),"checks":checks,"error":result.as_ref().err().map(|e|format!("{e:#}")),"scope":"已安装LocalSystem服务，普通账户真实命名管道与NTFS操作；只操作新建自有夹具"});
    fs::write(
        fixture.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    result
}
