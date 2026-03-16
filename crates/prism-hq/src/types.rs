use std::path::PathBuf;

pub fn cargo_bin(name: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin").join(name);
        if p.exists() {
            return p;
        }
    }
    name.into()
}
