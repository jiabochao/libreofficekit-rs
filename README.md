# libreofficekit-rs

[LibreOfficeKit](https://docs.libreoffice.org/libreofficekit.html) 的 Rust 绑定，专注于**文档转 PDF**。

- 运行时动态加载 LibreOffice：程序目录、动态库路径、用户配置目录都可以由调用方指定
- 支持 Windows / macOS / Linux（x86_64 与 aarch64）
- 随 App 发布**裁剪版** LibreOffice：`lok-pack export` 生成 macOS `LibreOffice.bundle` 或 Windows `libreoffice/` 目录，`lok-pack sign` 负责签名，`locate::app_program_dir()` 在运行时自动找到它
- `bundled` 特性：构建时自动准备运行时。先下载预编译包，没有就下载官方 LibreOffice 并在本地裁剪
- `lok-pack` 工具和 CI 工作流：从官方发行版生成裁剪包，在各平台测试后发布到 GitHub Releases

```
构建时  lok-pack export ──▶ src-tauri/lib/macos/LibreOffice.bundle   (lok-pack sign 签名)
                       └─▶ src-tauri/lib/windows/libreoffice/
运行时  Office::builder().program_dir(locate::app_program_dir()?)
          ──dlopen──▶ <App>.app/Contents/Frameworks/LibreOffice.bundle/Contents/Frameworks/libmergedlo.dylib
                      <exe dir>/libreoffice/program/mergedlo.dll
```

## 依赖

```toml
[dependencies]
libreofficekit-rs = { git = "https://github.com/jiabochao/libreofficekit-rs" }
```

库名是 `libreofficekit`（`use libreofficekit::...`）。

| 特性 | 作用 |
|------|------|
| *(无)* | 绑定本身；LibreOffice 的位置由你指定，或按 `LOK_PROGRAM_DIR`、App 包内、系统安装的顺序自动查找 |
| `bundled` | 构建时准备运行时，路径见 `libreofficekit::bundled_runtime_dir()` 和 `DEP_LIBREOFFICEKIT_RUNTIME_DIR` |

## 使用

```rust
use libreofficekit::{LoadOptions, Office, PdfOptions, PdfVersion};

let office = Office::builder()
    .install_dir("/path/to/libreoffice")          // 安装根目录、.app 或程序目录，自动解析
    // .program_dir(".../program")                // 或者精确指定程序目录
    // .library_path(".../program/libmergedlo.so") // 或者精确指定动态库
    .user_profile_dir("/path/to/cache/lo-profile")
    .build()?;

office.convert_to_pdf("a.docx", "a.pdf", &PdfOptions::default())?;

// PDF/A、加密、页码范围等
let opts = PdfOptions {
    version: Some(PdfVersion::PdfA2b),
    page_range: Some("1-3".into()),
    open_password: Some("secret".into()),
    ..Default::default()
};
office.convert_to_pdf("b.xlsx", "b.pdf", &opts)?;

// 打开带密码的文档
let load = LoadOptions { password: Some("pw".into()), ..Default::default() };
office.convert_to_pdf_with("c.docx", "c.pdf", &load, &PdfOptions::default())?;

// 任意格式（扩展名决定导出过滤器）
office.convert("d.doc", "d.docx", "docx", None)?;
```

`PdfOptions::extra` / `PdfOptions::set` 可以传任意 [PDF 导出参数](https://help.libreoffice.org/latest/en-US/text/shared/guide/pdf_params.html)。

### 必须知道的限制

- **每个进程只能初始化一次** LibreOfficeKit，`Office` drop 之后也不能再初始化（返回 `Error::AlreadyInitialized`）。把它放进 `OnceLock` 或 `Arc`，让它活到应用退出。
- 所有调用在内部串行执行。`Office` 是 `Send + Sync`，但同一时刻只会做一次转换。调用会阻塞，异步代码里请用 `spawn_blocking`。
- LibreOffice 在进程内运行，它崩溃时整个应用都会崩溃。如果要隔离，可以让应用以 worker 模式启动自身的子进程，在子进程里做转换。
- 默认以 `Batch=true` 加载文档，所有对话框都会被自动取消，因此不会卡住。只有加载失败时才会不带 `Batch` 重试一次，这样才能拿到密码请求（`Error::PasswordRequired` / `Error::WrongPassword`）。

## 在 Tauri 中使用（随 App 打包）

LibreOffice 不是几个单独的 dylib：裁剪后仍有约 2900 个文件，包括动态库、启动配置（`sofficerc`、`fundamentalrc`）、registry、UI 定义和字体，而且它按固定的相对路径查找这些文件。所以它要作为**一个独立目录**随 App 发布，不能像 pdfium 那样平铺：

| 平台 | 放进 `src-tauri/lib` 的内容 | 安装后的位置 |
|------|------|------|
| macOS | `lib/macos/LibreOffice.bundle` | `<App>.app/Contents/Frameworks/LibreOffice.bundle` |
| Windows | `lib/windows/libreoffice/` | exe 同级目录的 `libreoffice/` |

### 1. 生成

```sh
# 准备运行时（优先下载预编译包，没有就从官方包本地裁剪）并导出成 App 布局
cargo run --release -p libreofficekit-runtime --features builder --bin lok-pack -- \
  export ../opdf/src-tauri/lib/macos --platform macos-aarch64 --bundle-id com.jthinking.opdf.libreoffice
# Windows
lok-pack export ../opdf/src-tauri/lib/windows --platform windows-x86_64
```

`export` 在 macOS 上会删除 `Contents/MacOS` 下的可执行文件（App Store 包里的可执行文件必须都开启沙盒，进程内加载的 dylib 不受这个限制），删除 LibreOffice 原来的签名，并重写 `Info.plist`。

macOS 运行时按架构分开打包。要做 universal 包，需要先把两个架构的 dylib 用 `lipo` 合并，这一步目前没有实现。

### 2. Tauri 配置

`tauri.conf.json`：Tauri 的 `frameworks` 只接受 `.framework` 和 `.dylib`，所以用 `files` 复制整个 bundle。Tauri 不会签 `files` 里的内容，签名见下面第 4 步。

```json
"macOS": {
  "files": { "Frameworks/LibreOffice.bundle": "lib/macos/LibreOffice.bundle" }
}
```

`tauri.windows.conf.json` 里现有的 `"lib/windows": ""` 会递归复制，`libreoffice/` 子目录会一起带上，不需要改。

文件很多（约 2900 个，其中约 100 MB 是非动态库的数据文件），建议把 `lib/*/LibreOffice.bundle` 和 `lib/*/libreoffice` 加进 `.gitignore`，在 CI 或打包脚本里用 `lok-pack export` 生成。

### 3. 代码

```rust
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use libreofficekit::{Office, PdfOptions, locate};
use tauri::{AppHandle, Manager};

static OFFICE: OnceLock<Office> = OnceLock::new();

fn program_dir() -> Option<PathBuf> {
    // 打包后：<App>.app/Contents/Frameworks/LibreOffice.bundle 或 exe 旁边的 libreoffice/
    locate::app_program_dir().or_else(|| {
        // tauri dev：还没有打包，直接用 src-tauri/lib 下的导出目录
        let os = if cfg!(target_os = "macos") { "macos" } else { "windows" };
        locate::resolve_program_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("lib").join(os))
    })
}

/// 用户开启插件，或第一次转换时调用。启动约 0.2 秒，之后一直复用。
fn office(app: &AppHandle) -> Result<&'static Office, String> {
    if let Some(o) = OFFICE.get() {
        return Ok(o);
    }
    let dir = program_dir().ok_or("LibreOffice 运行时缺失")?;
    let profile = app.path().app_cache_dir().map_err(|e| e.to_string())?.join("lo-profile");
    let office = Office::builder().program_dir(dir).user_profile_dir(profile).build().map_err(|e| e.to_string())?;
    Ok(OFFICE.get_or_init(|| office))
}

#[tauri::command]
async fn convert_to_pdf(app: AppHandle, input: String, output: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        office(&app)?.convert_to_pdf(&input, &output, &PdfOptions::default()).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
```

`office()` 在 `spawn_blocking` 里执行，第一次初始化和后续转换都不会阻塞主线程。并发的第一次调用最多会有一个拿到 `AlreadyInitialized` 错误；如果介意，可以在外面加一把 `Mutex`。

### 4. 签名（macOS）

在 `tauri build` 之后，对已经复制进 App 的那份**从里到外**签名。不要对 App 用 `--deep`，否则主程序的 identifier 和 entitlements 会被打到每个 dylib 和嵌套 bundle 上：

```sh
APP=src-tauri/target/release/bundle/macos/oPDF.app
ID="Apple Distribution: ..."
lok-pack sign "$APP/Contents/Frameworks/LibreOffice.bundle" --identity "$ID"   # 每个 Mach-O → framework → bundle
codesign --force --options runtime --timestamp --sign "$ID" "$APP"/Contents/Frameworks/*.dylib
codesign --force --options runtime --timestamp --sign "$ID" --identifier <bundle id> --entitlements entitlements.plist "$APP"
codesign --verify --deep --strict --verbose=2 "$APP"
```

dylib 不需要 entitlements。开启 hardened runtime 时，library validation 只要求 dylib 和 App 的 Team ID 一致。Windows 上不需要额外签名：MSIX 由商店整体签名，LibreOffice 的 DLL 本身带有 TDF 的 Authenticode 签名，我们也没有修改文件内容。

## 上架 Mac App Store / Microsoft Store

把所有代码打包进 App 后，就不再违反 App Review Guidelines 2.5.2（禁止下载可执行代码）了。剩下的风险点：

1. **私有 API 扫描。** TDF 自己的 App Store 版用 `--enable-macosx-sandbox` 编译，这个选项会去掉不符合 App Store 要求的代码；官方 DMG 没有用这个选项。App Store Connect 的自动检查有可能报出私有 API 引用。建议尽早上传一个构建到 App Store Connect（TestFlight 处理阶段就会做这类校验），不要等到正式提审。如果被报出来，就需要用 `--enable-macosx-sandbox` 从源码编译 LibreOffice。
2. **沙盒里的运行时行为。** CI 测试不是在沙盒里跑的。需要在真正开启沙盒的构建里验证字体、临时文件，以及容器目录里的用户配置是否正常。
3. **签名。** 所有 Mach-O 文件都要用你的证书签名（`lok-pack sign`）。开启 hardened runtime 时，library validation 要求 dylib 的 Team ID 和 App 一致，所以不需要 `disable-library-validation`。
4. **体积。** macOS 安装包会增加约 100 MB（压缩后），安装后占用约 400 MB。

Microsoft Store 没有类似限制。LibreOffice 安装在 WindowsApps 只读目录下没有问题，因为用户配置写在 `app_cache_dir()` 里。

以上是我对政策的理解，不是 Apple 或 Microsoft 的官方结论。

## 运行时包

`libreofficekit-runtime` 决定使用哪个 LibreOffice 版本（`LO_VERSION = 26.2.6.3`，包修订 `REVISION = 1`）：

- 发布地址：`<base>/runtime-<LO_VERSION>-r<REVISION>/libreoffice-<LO_VERSION>-r<REVISION>-<platform>.tar.zst`，以及同名的 `.sha256` 文件
- `<base>` 默认是 `https://github.com/jiabochao/libreofficekit-rs/releases/download`。构建时（`lok-pack export`、`bundled`）会从这里下载预编译包，可以用环境变量 `LIBREOFFICEKIT_RUNTIME_BASE_URL` 覆盖（逗号分隔多个镜像）
- 发布后把各平台的哈希写进 `PINNED_SHA256`，校验就不再依赖同源的 `.sha256` 文件
- 包根目录下有 `libreofficekit-runtime.json`，记录版本、平台和程序目录

| 平台 | 官方包 | 解包方式 |
|------|--------|----------|
| linux-x86_64 / linux-aarch64 | `*_deb.tar.gz` | 纯 Rust 实现，只解出需要的 deb（跳过 Python、Base、Firebird、扩展、词典等） |
| windows-x86_64 / windows-aarch64 | `.msi` | Windows 上用 `msiexec /a`，其他系统用 `msiextract`（msitools） |
| macos-x86_64 / macos-aarch64 | `.dmg` | macOS 上用 `hdiutil` + `ditto`，其他系统用 `7z` |

解包后再按 `trim.rs` 的规则删除帮助、图库、模板、向导、Python/Java、图标主题、界面翻译、PDF 导入等文件。许可文件（`LICENSE*`、`NOTICE*`、`CREDITS*`）始终保留，这是 MPL-2.0 的要求。

在 linux-x86_64 上实测：

| | 大小 |
|--|--|
| 官方完整安装 | 730 MB（下载 208 MB） |
| 裁剪后 | 414 MB |
| 发布包 `.tar.zst` | **106 MB** |

### 生成与发布

```sh
# 打印版本、资产名和官方来源
cargo run -p libreofficekit-runtime --features builder --bin lok-pack -- info
# 为当前平台构建（--keep-lang zh-CN,ja 可以保留界面翻译）
cargo run --release -p libreofficekit-runtime --features builder --bin lok-pack -- build --out dist
# 只预览裁剪结果
lok-pack trim <dir> --platform windows-x86_64 --dry-run
# 从本地包或镜像安装
lok-pack install <dir> --archive dist/libreoffice-…tar.zst
```

`.github/workflows/runtime.yml` 在 6 个平台的原生 runner 上分别构建，安装后跑转换测试，推送 tag `runtime-26.2.6.3-r1` 后发布到 Releases。升级 LibreOffice 的步骤：修改 `LO_VERSION`（修订号重置为 1），推送对应的 tag。

### `bundled`（构建时准备）

```toml
libreofficekit-rs = { git = "...", features = ["bundled"] }
```

构建脚本会：

1. 复用缓存目录（`LIBREOFFICEKIT_CACHE_DIR`，默认 `~/.cache/libreofficekit-rs` 等）中已有的运行时；
2. 否则下载预编译包；
3. 预编译包不存在时，下载官方 LibreOffice 并在本地裁剪（`LIBREOFFICEKIT_FORCE_LOCAL_BUILD=1` 可以强制走这一步）。

其他环境变量：`LIBREOFFICEKIT_RUNTIME_DIR` 直接使用已经准备好的目录；`LIBREOFFICEKIT_OFFICIAL_MIRROR` 设置官方包镜像。

应用的 `build.rs` 可以通过 `DEP_LIBREOFFICEKIT_RUNTIME_DIR` 拿到这个目录，拷贝进 Tauri 的 resources。

## 测试

```sh
cargo test --workspace                      # 没有找到 LibreOffice 时，集成测试会跳过
LOK_PROGRAM_DIR=/path/to/runtime cargo test # 针对指定的运行时测试
cargo run --example convert -- --lo /path/to/runtime a.docx b.xlsx out/
```

Linux 运行时依赖系统库：`libnss3 libcups2 libcairo2 libX11 libXext libfontconfig libfreetype libglib2.0 libdbus-1 libavahi-client3`。

## 许可

本仓库代码使用 MIT 许可。LibreOffice 本身是 MPL-2.0，分发运行时包时请保留其中的许可文件，并注明源码地址：<https://www.libreoffice.org/download/source-code/>。
