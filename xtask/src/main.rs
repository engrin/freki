//! freki's release packaging: `cargo xtask dist [FORMAT...]`.
//!
//! Builds freki for the OS it runs on and packages it into `dist/`; the release workflow
//! (`.github/workflows/release.yml`) runs it on a Linux, a Windows and a macOS runner.
//! Uses only the standard library and the OS's own tools.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, exit};

const USAGE: &str = "\
usage: cargo xtask dist [FORMAT...]

Builds freki in release mode for this OS and packages it into dist/:
  Linux    tar (.tar.gz), deb, rpm, appimage
  Windows  zip
  macOS    dmg (a universal freki.app; needs the aarch64- and x86_64-apple-darwin targets)
Without FORMATs, all of this OS's formats are made.

File names carry $GITHUB_REF_NAME when it is set (on GitHub Actions), else v<version>.
deb needs dpkg-deb, rpm needs rpmbuild, appimage needs appimagetool (on PATH or in
$APPIMAGETOOL; $APPIMAGE_RUNTIME names the type-2 runtime file to embed).";

const FORMATS: &[&str] = if cfg!(target_os = "linux") {
    &["tar", "deb", "rpm", "appimage"]
} else if cfg!(windows) {
    &["zip"]
} else if cfg!(target_os = "macos") {
    &["dmg"]
} else {
    &[]
};

const SUMMARY: &str = "Game Boy, Game Boy Color and Super Game Boy emulator";
const DESCRIPTION: &str = "\
freki emulates the Game Boy, Game Boy Color and Super Game Boy, with a debugger,
link cable, Game Boy Printer and Game Boy Camera. It comes without games or boot ROMs.";
const HOMEPAGE: &str = "https://github.com/engrin/freki";
const MAINTAINER: &str = "engrin <122048+engrin@users.noreply.github.com>";
const BUNDLE_ID: &str = "io.github.engrin.freki";
const MACOS_MINIMUM: &str = "11.0";

type Result<T = ()> = std::result::Result<T, String>;

fn main() {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.split_first() {
        Some((command, formats))
            if command == "dist" && !formats.iter().any(|f| f.starts_with('-')) =>
        {
            dist(formats)
        }
        _ => {
            eprintln!("{USAGE}");
            exit(2);
        }
    };
    if let Err(err) = result {
        eprintln!("xtask: {err}");
        exit(1);
    }
}

fn dist(requested: &[String]) -> Result {
    if FORMATS.is_empty() {
        return Err("no release formats for this OS".into());
    }
    let formats: BTreeSet<&str> = if requested.is_empty() {
        FORMATS.iter().copied().collect()
    } else {
        let mut set = BTreeSet::new();
        for format in requested {
            let known = FORMATS.iter().find(|&&f| f == format);
            set.insert(*known.ok_or_else(|| {
                format!(
                    "unknown format {format} (this OS has: {})",
                    FORMATS.join(", ")
                )
            })?);
        }
        set
    };
    let dist = Dist::new()?;
    fresh_dir(&dist.out)?;
    fresh_dir(&dist.work)?;
    if cfg!(target_os = "linux") {
        dist.linux(&formats)?;
    } else if cfg!(windows) {
        dist.windows()?;
    } else {
        dist.macos()?;
    }
    eprintln!("\nPackages in {}:", dist.out.display());
    for entry in read_dir(&dist.out)? {
        eprintln!(
            "  {}",
            entry.file_name().unwrap_or_default().to_string_lossy()
        );
    }
    Ok(())
}

struct Dist {
    /// The workspace.
    root: PathBuf,
    /// The finished packages (`dist/`).
    out: PathBuf,
    /// Staging directories (`target/dist/`).
    work: PathBuf,
    /// The release name in file names: the tag or branch, or `v<version>`.
    label: String,
    /// The version (from a `v1.2.3` tag, else the workspace's), `-` turned into `~` for
    /// the packages' version ordering (1.0.0~rc1 comes before 1.0.0).
    version: String,
}

impl Dist {
    fn new() -> Result<Self> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .ok_or("no workspace directory")?
            .to_path_buf();
        let label = match env::var("GITHUB_REF_NAME") {
            Ok(name) if !name.is_empty() => name.replace(['/', '\\'], "-"),
            _ => format!("v{}", env!("CARGO_PKG_VERSION")),
        };
        let version = match label.strip_prefix('v') {
            Some(tag) if tag.starts_with(|c: char| c.is_ascii_digit()) => tag,
            _ => env!("CARGO_PKG_VERSION"),
        }
        .replace('-', "~");
        Ok(Self {
            out: root.join("dist"),
            work: root.join("target").join("dist"),
            root,
            label,
            version,
        })
    }

    fn cargo_build(&self, targets: &[&str]) -> Result {
        let mut cargo = Command::new(env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
        cargo
            .current_dir(&self.root)
            .args(["build", "--release", "--locked", "-p", "freki"]);
        for target in targets {
            cargo.args(["--target", target]);
        }
        if cfg!(target_os = "macos") && env::var_os("MACOSX_DEPLOYMENT_TARGET").is_none() {
            cargo.env("MACOSX_DEPLOYMENT_TARGET", MACOS_MINIMUM);
        }
        run(&mut cargo)
    }

    /// The release binary and freki's README and licence in a directory named like the package.
    fn plain_dir(&self, name: &str, binary: &str) -> Result<PathBuf> {
        let dir = self.work.join(name);
        fresh_dir(&dir)?;
        copy(
            &self.root.join("target/release").join(binary),
            &dir.join(binary),
        )?;
        copy(&self.root.join("README.md"), &dir.join("README.md"))?;
        copy(&self.root.join("LICENSE"), &dir.join("LICENSE"))?;
        Ok(dir)
    }

    fn linux(&self, formats: &BTreeSet<&str>) -> Result {
        self.cargo_build(&[])?;
        let name = format!("freki-{}-linux-x86_64", self.label);
        if formats.contains("tar") {
            self.plain_dir(&name, "freki")?;
            let archive = self.out.join(format!("{name}.tar.gz"));
            run(Command::new("tar")
                .arg("czf")
                .arg(&archive)
                .arg("-C")
                .arg(&self.work)
                .arg(&name))?;
        }
        if formats.iter().all(|f| *f == "tar") {
            return Ok(());
        }
        // The installed files (under /usr), shared by the deb, the rpm and the AppImage.
        let tree = self.work.join("tree");
        self.usr_tree(&tree)?;
        if formats.contains("deb") {
            self.deb(&tree, &name)?;
        }
        if formats.contains("rpm") {
            self.rpm(&tree, &name)?;
        }
        if formats.contains("appimage") {
            self.appimage(&tree, &name)?;
        }
        Ok(())
    }

    fn usr_tree(&self, tree: &Path) -> Result {
        fresh_dir(tree)?;
        let usr = tree.join("usr");
        let binary = usr.join("bin/freki");
        copy(&self.root.join("target/release/freki"), &binary)?;
        set_mode(&binary, 0o755)?;
        copy(
            &self.root.join("assets/freki.desktop"),
            &usr.join("share/applications/freki.desktop"),
        )?;
        copy(
            &self.root.join("assets/freki.svg"),
            &usr.join("share/icons/hicolor/scalable/apps/freki.svg"),
        )?;
        let doc = usr.join("share/doc/freki");
        copy(&self.root.join("README.md"), &doc.join("README.md"))?;
        copy(&self.root.join("LICENSE"), &doc.join("copyright"))
    }

    fn deb(&self, tree: &Path, name: &str) -> Result {
        let root = self.work.join("deb");
        fresh_dir(&root)?;
        copy_dir(tree, &root)?;
        let binary = fs::read(tree.join("usr/bin/freki")).map_err(|e| format!("freki: {e}"))?;
        let glibc = glibc_needed(&binary).ok_or("no glibc version in the binary")?;
        let control = format!(
            "Package: freki\n\
             Version: {version}\n\
             Architecture: amd64\n\
             Maintainer: {MAINTAINER}\n\
             Installed-Size: {size}\n\
             Depends: libc6 (>= {glibc}), libgcc-s1, libasound2 | libasound2t64, libudev1\n\
             Recommends: libgl1, libegl1, libxkbcommon0\n\
             Section: games\n\
             Priority: optional\n\
             Homepage: {HOMEPAGE}\n\
             Description: {SUMMARY}\n{description}\n",
            version = self.version,
            size = dir_size(tree)?.div_ceil(1024),
            description = indent(DESCRIPTION, " "),
        );
        write(&root.join("DEBIAN/control"), &control)?;
        // xz, not dpkg's zstd default on Ubuntu, which older Debian dpkg can't unpack.
        run(Command::new("dpkg-deb")
            .args(["--root-owner-group", "-Zxz", "--build"])
            .arg(&root)
            .arg(self.out.join(format!("{name}.deb"))))
    }

    fn rpm(&self, tree: &Path, name: &str) -> Result {
        let top = self.work.join("rpm");
        fresh_dir(&top)?;
        // A prebuilt binary: no debug package and no post-processing; rpmbuild finds
        // the libraries it needs by itself.
        let spec = format!(
            "%global debug_package %{{nil}}\n\
             %global __os_install_post %{{nil}}\n\
             %global _build_id_links none\n\
             Name: freki\n\
             Version: {version}\n\
             Release: 1\n\
             Summary: {SUMMARY}\n\
             License: MIT\n\
             URL: {HOMEPAGE}\n\
             Recommends: libglvnd-egl\n\
             \n\
             %description\n{DESCRIPTION}\n\
             \n\
             %install\n\
             mkdir -p %{{buildroot}}\n\
             cp -a '{tree}/usr' %{{buildroot}}/\n\
             \n\
             %files\n\
             /usr/bin/freki\n\
             /usr/share/applications/freki.desktop\n\
             /usr/share/icons/hicolor/scalable/apps/freki.svg\n\
             /usr/share/doc/freki\n",
            version = self.version,
            tree = tree.display(),
        );
        let spec_file = top.join("freki.spec");
        write(&spec_file, &spec)?;
        run(Command::new("rpmbuild")
            .arg("-bb")
            .arg("--define")
            .arg(format!("_topdir {}", top.display()))
            .arg("--define")
            .arg(format!("_rpmdir {}", self.out.display()))
            .arg("--define")
            .arg(format!("_build_name_fmt {name}.rpm"))
            .arg(spec_file))
    }

    fn appimage(&self, tree: &Path, name: &str) -> Result {
        let app_dir = self.work.join("freki.AppDir");
        fresh_dir(&app_dir)?;
        copy_dir(tree, &app_dir)?;
        copy(
            &self.root.join("assets/freki.desktop"),
            &app_dir.join("freki.desktop"),
        )?;
        copy(
            &self.root.join("assets/freki.svg"),
            &app_dir.join("freki.svg"),
        )?;
        symlink("usr/bin/freki", &app_dir.join("AppRun"))?;
        symlink("freki.svg", &app_dir.join(".DirIcon"))?;
        let tool = env::var_os("APPIMAGETOOL").unwrap_or_else(|| "appimagetool".into());
        let mut command = Command::new(tool);
        // ARCH names the runtime's architecture; extract-and-run lets appimagetool (an
        // AppImage itself) start without FUSE.
        command
            .env("ARCH", "x86_64")
            .env("APPIMAGE_EXTRACT_AND_RUN", "1");
        if let Some(runtime) = env::var_os("APPIMAGE_RUNTIME") {
            command.arg("--runtime-file").arg(runtime);
        }
        run(command
            .arg(&app_dir)
            .arg(self.out.join(format!("{name}.AppImage"))))
    }

    fn windows(&self) -> Result {
        self.cargo_build(&[])?;
        let name = format!("freki-{}-windows-x86_64", self.label);
        let dir = self.plain_dir(&name, "freki.exe")?;
        let zip = self.out.join(format!("{name}.zip"));
        run(Command::new("powershell")
            .args(["-NoProfile", "-NonInteractive", "-Command"])
            .arg(format!(
                "Compress-Archive -Path '{}' -DestinationPath '{}' -Force",
                dir.display(),
                zip.display()
            )))
    }

    fn macos(&self) -> Result {
        const TARGETS: [&str; 2] = ["aarch64-apple-darwin", "x86_64-apple-darwin"];
        // Both in one cargo call, so they build in parallel.
        self.cargo_build(&TARGETS)?;
        let image = self.work.join("dmg");
        fresh_dir(&image)?;
        let app = image.join("freki.app");
        let contents = app.join("Contents");
        mkdir(&contents.join("MacOS"))?;
        mkdir(&contents.join("Resources"))?;
        let mut lipo = Command::new("lipo");
        lipo.arg("-create")
            .arg("-output")
            .arg(contents.join("MacOS/freki"));
        for target in TARGETS {
            lipo.arg(self.root.join("target").join(target).join("release/freki"));
        }
        run(&mut lipo)?;
        // Bundle versions are numbers only: 1.0.0~rc1 → 1.0.0.
        let version = self.version.split('~').next().unwrap_or_default();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>freki</string>
  <key>CFBundleDisplayName</key><string>freki</string>
  <key>CFBundleIdentifier</key><string>{BUNDLE_ID}</string>
  <key>CFBundleExecutable</key><string>freki</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>{version}</string>
  <key>CFBundleVersion</key><string>{version}</string>
  <key>LSMinimumSystemVersion</key><string>{MACOS_MINIMUM}</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.games</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSCameraUsageDescription</key><string>freki can use a camera as the Game Boy Camera's sensor.</string>
</dict>
</plist>
"#
        );
        let plist_file = contents.join("Info.plist");
        write(&plist_file, &plist)?;
        run(Command::new("plutil").arg("-lint").arg(&plist_file))?;
        // Not notarized (no Apple Developer account): signed ad hoc only.
        run(Command::new("codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(&app))?;
        copy(&self.root.join("README.md"), &image.join("README.md"))?;
        copy(&self.root.join("LICENSE"), &image.join("LICENSE"))?;
        symlink("/Applications", &image.join("Applications"))?;
        let name = format!("freki-{}-macos-universal", self.label);
        run(Command::new("hdiutil")
            .args([
                "create",
                "-volname",
                "freki",
                "-format",
                "UDZO",
                "-ov",
                "-srcfolder",
            ])
            .arg(&image)
            .arg(self.out.join(format!("{name}.dmg"))))
    }
}

/// The newest `GLIBC_x.y` symbol version the binary asks for.
fn glibc_needed(binary: &[u8]) -> Option<String> {
    const TAG: &[u8] = b"GLIBC_";
    let mut newest: Option<Vec<u32>> = None;
    for start in 0..binary.len().saturating_sub(TAG.len()) {
        if &binary[start..start + TAG.len()] != TAG {
            continue;
        }
        let rest = &binary[start + TAG.len()..];
        let end = rest
            .iter()
            .position(|b| !b.is_ascii_digit() && *b != b'.')
            .unwrap_or(rest.len());
        let parts: Option<Vec<u32>> = std::str::from_utf8(&rest[..end])
            .ok()?
            .split('.')
            .map(|p| p.parse().ok())
            .collect();
        if let Some(parts) = parts.filter(|p| p.len() >= 2)
            && newest.as_ref().is_none_or(|n| parts > *n)
        {
            newest = Some(parts);
        }
    }
    newest.map(|n| n.iter().map(u32::to_string).collect::<Vec<_>>().join("."))
}

fn indent(text: &str, prefix: &str) -> String {
    text.lines()
        .map(|line| format!("{prefix}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn run(command: &mut Command) -> Result {
    eprintln!("+ {command:?}");
    let program = command.get_program().to_string_lossy().into_owned();
    let status = command.status().map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound => format!("{program} not found (is it installed?)"),
        _ => format!("{program}: {e}"),
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{program} failed ({status})"))
    }
}

fn fresh_dir(dir: &Path) -> Result {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    mkdir(dir)
}

fn mkdir(dir: &Path) -> Result {
    fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))
}

fn read_dir(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut paths: Vec<PathBuf> = entries.filter_map(|e| e.ok().map(|e| e.path())).collect();
    paths.sort();
    Ok(paths)
}

fn copy(from: &Path, to: &Path) -> Result {
    if let Some(parent) = to.parent() {
        mkdir(parent)?;
    }
    fs::copy(from, to)
        .map(drop)
        .map_err(|e| format!("{}: {e}", from.display()))
}

/// Copies a directory's contents (files and directories) into another directory.
fn copy_dir(from: &Path, to: &Path) -> Result {
    for path in read_dir(from)? {
        let target = to.join(path.file_name().unwrap_or_default());
        if path.is_dir() {
            mkdir(&target)?;
            copy_dir(&path, &target)?;
        } else {
            copy(&path, &target)?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> Result<u64> {
    let mut size = 0;
    for path in read_dir(dir)? {
        size += if path.is_dir() {
            dir_size(&path)?
        } else {
            fs::metadata(&path)
                .map_err(|e| format!("{}: {e}", path.display()))?
                .len()
        };
    }
    Ok(size)
}

fn write(path: &Path, contents: &str) -> Result {
    if let Some(parent) = path.parent() {
        mkdir(parent)?;
    }
    fs::write(path, contents).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> Result {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) -> Result {
    Ok(())
}

#[cfg(unix)]
fn symlink(target: &str, link: &Path) -> Result {
    std::os::unix::fs::symlink(target, link).map_err(|e| format!("{}: {e}", link.display()))
}

#[cfg(not(unix))]
fn symlink(_: &str, link: &Path) -> Result {
    Err(format!(
        "{}: symbolic links need a Unix system",
        link.display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_glibc_version() {
        let binary = b"\0GLIBC_2.2.5\0GLIBC_2.34\0GLIBC_PRIVATE\0GLIBC_2.17\0";
        assert_eq!(glibc_needed(binary).as_deref(), Some("2.34"));
        assert_eq!(glibc_needed(b"no versions"), None);
    }
}
