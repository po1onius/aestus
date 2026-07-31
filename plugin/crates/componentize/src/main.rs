use std::{env, fs, path::Path};

use anyhow::{Context, Result, bail};
use wit_component::ComponentEncoder;

/// 把 `wit-bindgen` 生成的 canonical ABI core module 封装成 Dashboard 接受的
/// WebAssembly Component。构建工具放在 workspace 内，避免要求部署机全局安装
/// `cargo-component` 或 `wasm-tools`。
fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let Some(input_path) = args.next() else {
        bail!("用法: gpt-plugin-componentize <core-wasm> <component-wasm>");
    };
    let Some(output_path) = args.next() else {
        bail!("用法: gpt-plugin-componentize <core-wasm> <component-wasm>");
    };
    if args.next().is_some() {
        bail!("gpt-plugin-componentize 只接受输入和输出两个路径参数");
    }

    let input_path = Path::new(&input_path);
    let output_path = Path::new(&output_path);
    let module = fs::read(input_path)
        .with_context(|| format!("读取 core WASM 失败: {}", input_path.display()))?;
    let component = ComponentEncoder::default()
        .validate(true)
        .module(&module)
        .context("core WASM 不包含有效的 Component Type 信息")?
        .encode()
        .context("编码 WebAssembly Component 失败")?;
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("创建输出目录失败: {}", parent.display()))?;
    }
    fs::write(output_path, component)
        .with_context(|| format!("写入 Component 失败: {}", output_path.display()))?;
    println!("已生成插件组件: {}", output_path.display());
    Ok(())
}
