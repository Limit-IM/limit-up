use std::{
    io::{ErrorKind, Read, Write},
    process::{Child, Command, Stdio},
};

use super::{Error, Result};

trait PkgManager {
    fn install(&self, pkgs: &str) -> String;
    fn uninstall(&self, pkgs: &str) -> String;
    fn name(&self) -> &'static str;
}

macro_rules! impl_pkg_manager {
    ($class:ident, $name:expr, $install:expr, $uninstall:expr, $flag:expr) => {
        pub struct $class;

        impl PkgManager for $class {
            fn install(&self, pkgs: &str) -> String {
                format!(
                    concat!($name, " ", $install, " {} ", $flag, " && exit\n"),
                    pkgs
                )
            }

            fn uninstall(&self, pkgs: &str) -> String {
                format!(
                    concat!($name, " ", $uninstall, " {} ", $flag, " && exit\n"),
                    pkgs
                )
            }

            fn name(&self) -> &'static str {
                $name
            }
        }
    };
}

#[cfg(target_family = "unix")]
impl_pkg_manager!(Apt, "apt", "install", "remove", "-y");
#[cfg(target_family = "unix")]
impl_pkg_manager!(Dnf, "dnf", "install", "remove", "-y");
#[cfg(target_family = "unix")]
impl_pkg_manager!(Pacman, "pacman", "-S", "-Rns", "--noconfirm");
#[cfg(target_family = "unix")]
impl_pkg_manager!(Zypper, "zypper", "install", "remove", "-y");
#[cfg(target_family = "unix")]
impl_pkg_manager!(Apk, "apk", "add", "del", ""); // apk does not need

macro_rules! boxed_mgrs {
    ($($mgr:ident),+) => {
        vec![$(Box::new($mgr {})),+]
    };
}

pub struct PackageManager {
    mgr: Box<dyn PkgManager + Send + Sync>,
    proc: Child,
}

impl PackageManager {
    #[cfg(target_family = "unix")]
    pub fn new_with_passwd(passwd: impl AsRef<str>) -> Result<PackageManager> {
        use std::os::fd::AsRawFd;

        use nix::{
            sys::{
                select::{select, FdSet},
                time::{TimeVal, TimeValLike},
            },
            unistd::Uid,
        };

        let mut root_proc = Command::new("/usr/bin/su")
            .args(["-s", "/usr/bin/bash"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let mut buf = [0; 16];

        if !passwd.as_ref().is_empty() && !Uid::effective().is_root() {
            root_proc
                .stdin
                .as_mut()
                .unwrap()
                .write_all(format!("{}\n", passwd.as_ref()).as_bytes())?;

            root_proc.stderr.as_mut().unwrap().read(&mut buf)?;
            while try_read!(root_proc.stderr, buf)? != 0 {}
        }

        loop {
            root_proc.stdin.as_mut().unwrap().write_all(b"whoami\n")?;

            let mut fdset = FdSet::new();
            fdset.insert(root_proc.stdout.as_ref().unwrap().as_raw_fd());
            fdset.insert(root_proc.stderr.as_ref().unwrap().as_raw_fd());

            select(
                fdset.highest().unwrap() + 1,
                &mut fdset,
                None,
                None,
                &mut TimeVal::seconds(5),
            )
            .map_err(|e| Error::IoError(e.into()))?;

            if fdset.contains(root_proc.stdout.as_ref().unwrap().as_raw_fd()) {
                root_proc.stdout.as_mut().unwrap().read(&mut buf)?;
                break;
            } else if fdset.contains(root_proc.stderr.as_ref().unwrap().as_raw_fd()) {
                return Err(Error::IoError(ErrorKind::PermissionDenied.into()));
            }
        }

        let mgrs: Vec<Box<dyn PkgManager + Send + Sync>> =
            boxed_mgrs![Apt, Dnf, Pacman, Zypper, Apk];

        mgrs.into_iter()
            .find(|mgr| Command::new(mgr.name()).output().is_ok())
            .map(|mgr| PackageManager {
                mgr,
                proc: root_proc,
            })
            .ok_or(Error::NotSupported)
    }

    pub fn install(mut self, pkgs: impl IntoIterator<Item = impl Into<String>>) -> Result<Child> {
        self.proc
            .stdin
            .as_mut()
            .unwrap()
            .write_all(
                self.mgr
                    .install(
                        pkgs.into_iter()
                            .map(|p| p.into())
                            .collect::<Vec<String>>()
                            .join(" ")
                            .as_str(),
                    )
                    .as_bytes(),
            )
            .map_err(|e| Error::IoError(e))?;

        Ok(self.proc)
    }

    pub fn uninstall(mut self, pkgs: impl IntoIterator<Item = impl Into<String>>) -> Result<Child> {
        self.proc
            .stdin
            .as_mut()
            .unwrap()
            .write_all(
                self.mgr
                    .uninstall(
                        pkgs.into_iter()
                            .map(|p| p.into())
                            .collect::<Vec<String>>()
                            .join(" ")
                            .as_str(),
                    )
                    .as_bytes(),
            )
            .map_err(|e| Error::IoError(e))?;

        Ok(self.proc)
    }

    pub fn name(&self) -> &'static str {
        self.mgr.name()
    }
}

#[cfg(test)]
mod tests {
    use std::env;

    use super::PackageManager;

    #[test]
    fn pkgmgr_test() {
        let passwd = env::var("PASSWD").unwrap_or_default();

        let res = PackageManager::new_with_passwd(&passwd)
            .map(|mgr| {
                println!("package manager: {}", mgr.name());
                mgr
            })
            .unwrap()
            .install(["cowsay"])
            .unwrap()
            .wait_with_output()
            .unwrap();

        println!("install: {}", res.status);
        println!("stdout:\n{}\n", String::from_utf8(res.stdout).unwrap());
        println!("stderr:\n{}\n", String::from_utf8(res.stderr).unwrap());

        let res = PackageManager::new_with_passwd(&passwd)
            .unwrap()
            .uninstall(["cowsay"])
            .unwrap()
            .wait_with_output()
            .unwrap();

        println!("uninstall: {}", res.status);
        println!("stdout:\n{}\n", String::from_utf8(res.stdout).unwrap());
        println!("stderr:\n{}\n", String::from_utf8(res.stderr).unwrap());
    }
}