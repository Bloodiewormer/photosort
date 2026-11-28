// Gives the Windows executable its icon and name, as Explorer and the desktop shortcut show them.
fn main() {
    println!("cargo:rerun-if-changed=../../assets/icon.ico");
    #[cfg(windows)]
    {
        let mut res = winresource::WindowsResource::new();
        res.set_icon("../../assets/icon.ico");
        res.set("ProductName", "Hojeo");
        res.set("FileDescription", "Hojeo");
        res.compile()
            .expect("no se pudo incrustar el icono en el ejecutable");
    }
}
