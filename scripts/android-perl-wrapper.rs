//! Translate the OpenSSL build's Windows --prefix path for MSYS Perl.
//! openssl-src emits E:/... on an MSVC host; Unix-target OpenSSL Configure
//! checks that path using Unix File::Spec and rejects it as relative. The
//! Git's Perl/shell environment uses /e for Windows drives.
use std::{env, ffi::OsString, path::{Path, PathBuf}, process::{Command, exit}};

fn unix_path(path: &Path) -> Option<String> {
    let path = path.to_str()?.replace('\\', "/");
    let bytes = path.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' || bytes[2] != b'/' {
        return None;
    }
    Some(format!("/{}/{}", (bytes[0] as char).to_ascii_lowercase(), &path[3..]))
}

fn perl_modules(root: &Path) -> Option<String> {
    let modules = root.join(".local/android-toolchain/perl-modules");
    let source = modules.join("perl-5.38.2");
    let mut paths = vec![source.join("lib")];
    for section in ["cpan", "dist"] {
        for entry in std::fs::read_dir(source.join(section)).ok()?.flatten() {
            let path = entry.path().join("lib");
            if path.is_dir() { paths.push(path); }
        }
    }
    paths.push(modules.join("Locale-Maketext-Simple-0.21/lib"));
    paths.iter().map(|path| unix_path(path)).collect::<Option<Vec<_>>>().map(|paths| paths.join(":"))
}

fn main() {
    let Some(perl) = env::var_os("HTTP_CAPTURE_UNIX_PERL") else {
        eprintln!("HTTP_CAPTURE_UNIX_PERL is not set");
        exit(2);
    };
    let mut args: Vec<OsString> = env::args_os().skip(1).map(|arg| {
        let Some(text) = arg.to_str() else { return arg };
        let Some(path) = text.strip_prefix("--prefix=") else { return arg };
        let bytes = path.as_bytes();
        if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':'
            || !matches!(bytes[2], b'/' | b'\\') {
            return arg;
        }
        OsString::from(format!("--prefix=/{}/{}", (bytes[0] as char).to_ascii_lowercase(),
                               path[3..].replace('\\', "/")))
    }).collect();
    if args.first().is_some_and(|arg| arg == "./Configure") {
        // MSYS Perl and native Windows Make disagree on shell quoting for the
        // generated ARM assembler recipes. Use OpenSSL's portable C routines
        // for this local cross build; Linux CI keeps assembly enabled.
        args.push("no-asm".into());
    }
    let mut command = Command::new(perl);
    command.args(args);
    if let Some(paths) = env::var_os("HTTP_CAPTURE_PROJECT_ROOT")
        .and_then(|root| perl_modules(&PathBuf::from(root))) {
        // Build the Unix path list here, after Cygwin's environment conversion.
        command.env("PERL5LIB", paths);
    }
    match command.status() {
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(error) => { eprintln!("Could not start Unix Perl: {error}"); exit(2) }
    }
}
