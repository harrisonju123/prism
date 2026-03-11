use std::path::PathBuf;

fn cargo_bin(name: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        let p = PathBuf::from(home).join(".cargo/bin").join(name);
        if p.exists() {
            return p;
        }
    }
    name.into()
}

pub fn uh_binary() -> PathBuf {
    cargo_bin("uh")
}

pub fn prism_binary() -> PathBuf {
    cargo_bin("prism")
}
