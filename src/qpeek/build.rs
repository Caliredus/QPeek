use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let config = slint_build::CompilerConfiguration::new().with_style("fluent".into());
    slint_build::compile_with_config("ui/app.slint", config).expect("Slint derlenemedi");
    embed_provider_icons();
    embed_provider_meta();
    embed_windows_icon();
}

fn embed_provider_meta() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let vendor = manifest.join("../../vendor/Win-CodexBar/rust/src");
    let provider_path = vendor.join("core/provider.rs");
    let api_keys_path = vendor.join("settings/api_keys.rs");
    println!("cargo:rerun-if-changed={}", provider_path.display());
    println!("cargo:rerun-if-changed={}", api_keys_path.display());

    let provider = std::fs::read_to_string(provider_path).expect("provider.rs okunamadı");
    let api_keys = std::fs::read_to_string(api_keys_path)
        .expect("api_keys.rs okunamadı")
        .replace("\r\n", "\n");

    let (_, cli_name) = provider
        .split_once("fn cli_name(&self)")
        .expect("ProviderId::cli_name bulunamadı");
    let (cli_name, _) = cli_name.split_once("\n    }").unwrap();
    let cli_names: HashMap<_, _> = cli_name
        .lines()
        .filter_map(|line| {
            let (variant, id) = line.trim().strip_prefix("ProviderId::")?.split_once("=>")?;
            let id = id.trim().strip_prefix('"')?.strip_suffix("\",")?;
            Some((variant.trim(), id))
        })
        .collect();

    let (_, configs) = api_keys
        .split_once("fn get_api_key_providers()")
        .expect("get_api_key_providers bulunamadı");
    let (configs, _) = configs.split_once("\n}").unwrap();
    let mut metadata = BTreeMap::new();
    for config in configs.split("ProviderConfigInfo {").skip(1) {
        let variant = provider_config_field(config, "id:")
            .split_once(',')
            .unwrap()
            .0
            .trim()
            .strip_prefix("ProviderId::")
            .unwrap();
        let id = cli_names
            .get(variant)
            .copied()
            .unwrap_or_else(|| panic!("ProviderId::{variant} için CLI kimliği bulunamadı"));
        let required: bool = provider_config_field(config, "requires_api_key:")
            .split_once(',')
            .unwrap()
            .0
            .trim()
            .parse()
            .unwrap();
        // GitHub sign-in is the primary flow. The token is an optional fallback.
        let required = required && id != "copilot";
        let env_var = provider_config_string(provider_config_field(config, "api_key_env_var:"));
        let help = provider_config_string(provider_config_field(config, "api_key_help:"));
        assert!(
            metadata.insert(id, (required, env_var, help)).is_none(),
            "Yinelenen sağlayıcı kimliği: {id}"
        );
    }
    assert!(metadata.len() >= 45, "En az 45 sağlayıcı bekleniyordu");

    let mut arms = String::new();
    for (id, (required, env_var, help)) in metadata {
        writeln!(
            arms,
            "        {id:?} => Some(ApiKeyMeta {{ required: {required}, env_var: {env_var:?}, help: {help:?} }}),"
        )
        .unwrap();
    }
    let code = format!(
        r#"#[derive(Debug, Clone, Copy)]
pub struct ApiKeyMeta {{
    pub required: bool,
    pub env_var: &'static str,
    pub help: &'static str,
}}

/// Sağlayıcı kimliğine göre derleme zamanı API anahtarı bilgisi (`Win-CodexBar` kaynağından).
pub fn api_key_meta(id: &str) -> Option<ApiKeyMeta> {{
    match id {{
{arms}        _ => None,
    }}
}}
"#
    );
    let out = PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("provider_meta.rs");
    std::fs::write(out, code).expect("provider_meta.rs yazılamadı");
}

fn provider_config_field<'a>(config: &'a str, field: &str) -> &'a str {
    let (offset, _) = config
        .match_indices(field)
        .find(|(offset, _)| {
            config[..*offset]
                .rsplit('\n')
                .next()
                .unwrap()
                .trim()
                .is_empty()
        })
        .unwrap_or_else(|| panic!("ProviderConfigInfo alanı bulunamadı: {field}"));
    config[offset + field.len()..].trim_start()
}

fn provider_config_string(value: &str) -> String {
    if value.starts_with("None,") {
        return String::new();
    }
    let literal = value
        .strip_prefix("Some(")
        .and_then(|value| value.trim_start().strip_prefix('"'))
        .expect("ProviderConfigInfo alanında Some veya None bekleniyordu");

    let mut chars = literal.chars();
    let mut decoded = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return decoded,
            '\\' => match chars.next() {
                // Rust skips the newline and subsequent ASCII whitespace after a backslash.
                Some('\n') => {
                    chars = chars
                        .as_str()
                        .trim_start_matches([' ', '\t', '\n', '\r'])
                        .chars();
                }
                Some('n') => decoded.push('\n'),
                Some('r') => decoded.push('\r'),
                Some('t') => decoded.push('\t'),
                Some('0') => decoded.push('\0'),
                Some(escaped @ ('\\' | '"' | '\'')) => decoded.push(escaped),
                Some('x') => {
                    let hex: String = chars.by_ref().take(2).collect();
                    decoded.push(char::from(u8::from_str_radix(&hex, 16).unwrap()));
                }
                Some('u') => {
                    assert_eq!(chars.next(), Some('{'));
                    let hex: String = chars
                        .by_ref()
                        .take_while(|&ch| ch != '}')
                        .filter(|&ch| ch != '_')
                        .collect();
                    decoded.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
                }
                escape => panic!("Geçersiz Rust dize kaçışı: {escape:?}"),
            },
            _ => decoded.push(ch),
        }
    }
    panic!("Kapanmamış ProviderConfigInfo dizesi");
}

/// Çekirdeğin sağlayıcı logolarını (vendor/Win-CodexBar/rust/assets/icons/ProviderIcon-*.svg)
/// exe'ye gömer: `provider_icon_svg("claude")` gibi bir eşleme üretir.
fn embed_provider_icons() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = manifest.join("../../vendor/Win-CodexBar/rust/assets/icons");
    println!("cargo:rerun-if-changed={}", dir.display());

    let mut arms = String::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for path in files {
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or_default();
            let Some(id) = name.strip_prefix("ProviderIcon-").and_then(|n| n.strip_suffix(".svg")) else {
                continue;
            };
            let abs = path.canonicalize().unwrap();
            let abs = abs.to_string_lossy().replace('\\', "/");
            let abs = abs.strip_prefix("//?/").unwrap_or(&abs);
            writeln!(arms, "        {id:?} => Some(include_bytes!({abs:?})),").unwrap();
        }
    }
    let code = format!(
        "/// Sağlayıcı kimliğine göre gömülü SVG logo (beyaz, tek renk).\n\
         pub fn provider_icon_svg(id: &str) -> Option<&'static [u8]> {{\n    match id {{\n{arms}        _ => None,\n    }}\n}}\n"
    );
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("provider_icons.rs");
    std::fs::write(out, code).unwrap();
}

/// Windows `.exe` dosya ikonunu (assets/qpeek.ico) harici crate indirmeden Windows SDK `rc.exe` ile gömer.
fn embed_windows_icon() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let ico = manifest.join("assets/qpeek.ico");
    println!("cargo:rerun-if-changed={}", ico.display());
    if !ico.is_file() {
        return;
    }
    let Some(rc) = find_rc_exe() else {
        println!("cargo:warning=rc.exe bulunamadı; QPeek.exe ikonu gömülmedi");
        return;
    };
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let rc_file = out.join("qpeek.rc");
    let res_file = out.join("qpeek.res");
    let ico_escaped = ico.to_string_lossy().replace('\\', "\\\\");
    std::fs::write(&rc_file, format!("1 ICON \"{ico_escaped}\"\n")).expect("qpeek.rc yazılamadı");

    let status = Command::new(rc)
        .args(["/nologo", "/fo"])
        .arg(&res_file)
        .arg(&rc_file)
        .status()
        .expect("rc.exe çalıştırılamadı");
    assert!(status.success(), "rc.exe başarısız oldu");
    println!("cargo:rustc-link-arg-bins={}", res_file.display());
}

fn find_rc_exe() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("PATH")
        && let Some(found) = std::env::split_paths(&path)
            .map(|d| d.join("rc.exe"))
            .find(|p| p.is_file())
    {
        return Some(found);
    }
    let kits = PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(kits)
        .ok()?
        .flatten()
        .map(|e| e.path().join("x64/rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    candidates.sort();
    candidates.pop()
}
