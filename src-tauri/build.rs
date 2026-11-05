use sha2::{Digest, Sha256};
use std::{env, fs::File, io, path::PathBuf};

fn main() {
    // 安装入口只接受与本次 GUI 构建绑定的服务程序，拒绝替换后的用户目录载荷。
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("缺少构建目录"));
    let service = manifest.join("../target/release/foldsentinel-service.exe");
    println!("cargo:rerun-if-changed={}", service.display());
    let digest = match File::open(&service) {
        Ok(mut file) => {
            let mut hash = Sha256::new();
            io::copy(&mut file, &mut hash).expect("读取服务安装载荷失败");
            format!("{:x}", hash.finalize())
        }
        Err(error)
            if error.kind() == io::ErrorKind::NotFound
                && env::var("PROFILE").as_deref() != Ok("release") =>
        {
            // 干净 checkout 的单测和预览不依赖 release 产物；安装命令会拒绝空散列。
            String::new()
        }
        Err(error) => panic!(
            "缺少可验证的服务安装载荷，请通过 scripts/package.ps1 构建：{}：{error}",
            service.display()
        ),
    };
    println!("cargo:rustc-env=FOLDSENTINEL_SERVICE_SHA256={digest}");
    tauri_build::build();
}
