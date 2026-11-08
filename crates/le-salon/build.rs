// GPUI (backend X11) demande `-lxkbcommon-x11`, mais Fedora ne fournit que
// `libxkbcommon-x11.so.0` sans le lien de développement si le paquet -devel
// n'est pas installé. On fabrique un lien local pour que `cargo build` marche
// sans root, et on l'ajoute au chemin de recherche de l'éditeur de liens.
#[cfg(target_os = "linux")]
fn main() {
    use std::path::PathBuf;

    println!("cargo:rerun-if-changed=build.rs");
    let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    let link_dir = out_dir.join("native-libs");
    std::fs::create_dir_all(&link_dir).expect("création du dossier de liens");
    let link = link_dir.join("libxkbcommon-x11.so");
    if !link.exists() {
        let candidates = [
            "/usr/lib64/libxkbcommon-x11.so.0",
            "/usr/lib/libxkbcommon-x11.so.0",
            "/lib64/libxkbcommon-x11.so.0",
            "/lib/libxkbcommon-x11.so.0",
        ];
        for candidate in candidates {
            if std::path::Path::new(candidate).exists() {
                #[cfg(unix)]
                let _ = std::os::unix::fs::symlink(candidate, &link);
                break;
            }
        }
    }
    println!("cargo:rustc-link-search=native={}", link_dir.display());
}

#[cfg(not(target_os = "linux"))]
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
}
