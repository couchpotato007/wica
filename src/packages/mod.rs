//! Install instructions for tools that aren't installed.
//!
//! Package managers are pure data (see `builtin.toml`), so supporting a new
//! distro means adding a table there or in the user's config, not code.

use std::collections::HashMap;
use std::io::{IsTerminal, Read};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::sources::path;

const BUILTIN: &str = include_str!("builtin.toml");
/// Give up on a lookup after this long (some managers refresh metadata).
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(8);
/// Re-run lookups after this long; package names rarely change.
const CACHE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Package searches go stale faster (repositories update) and are bigger.
const SEARCH_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Deserialize)]
struct Config {
    #[serde(default, rename = "manager")]
    managers: Vec<Manager>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Manager {
    pub name: String,
    pub detect: String,
    pub install: String,
    #[serde(rename = "try")]
    pub try_cmd: Option<String>,
    #[serde(default)]
    pub lookup: Vec<Lookup>,
    /// Search package names, for `wica name`.
    pub search: Option<Search>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Lookup {
    pub command: Vec<String>,
    pub pattern: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Search {
    pub command: Vec<String>,
    /// Applied to the whole output; group 1 = package, optional group 2 = description.
    pub pattern: String,
    /// Seconds; some searches (nix) are slow the first time.
    pub timeout: Option<u64>,
}

/// A package found by a manager's search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Found {
    pub name: String,
    pub description: Option<String>,
}

/// How to get one tool with one package manager.
#[derive(Debug, Clone)]
pub struct Hint {
    pub manager: String,
    pub install: String,
    pub try_cmd: Option<String>,
    /// No lookup confirmed the package name; we assumed package = command.
    pub guessed: bool,
}

pub fn user_config() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("wica/package-managers.toml"))
}

/// All configured managers: the user's first, then built-ins they don't replace.
pub fn configured() -> Result<Vec<Manager>> {
    let builtin: Config = toml::from_str(BUILTIN).context("built-in package manager table")?;
    let mut managers = Vec::new();
    if let Some(file) = user_config().filter(|f| f.exists()) {
        let text = std::fs::read_to_string(&file)?;
        let user: Config = toml::from_str(&text).with_context(|| format!("parsing {}", file.display()))?;
        managers.extend(user.managers);
    }
    for m in builtin.managers {
        if !managers.iter().any(|u: &Manager| u.name == m.name) {
            managers.push(m);
        }
    }
    Ok(managers)
}

pub fn is_available(m: &Manager) -> bool {
    path::find(&m.detect).is_some()
}

pub struct Packages {
    managers: Vec<Manager>,
    cache: Cache,
}

impl Packages {
    /// Package managers available on this system.
    pub fn detect() -> Result<Self> {
        let managers = configured()?.into_iter().filter(is_available).collect();
        Ok(Self { managers, cache: Cache::load() })
    }

    /// Install hints for several commands at once; lookups run in parallel.
    pub fn hints(&mut self, commands: &[&str]) -> HashMap<String, Vec<Hint>> {
        let now = now();
        // Run the lookups that aren't cached.
        let todo: Vec<(&Manager, &str)> = self
            .managers
            .iter()
            .flat_map(|m| commands.iter().map(move |c| (m, *c)))
            .filter(|(m, c)| !m.lookup.is_empty() && self.cache.get(&m.name, c, now).is_none())
            .collect();
        let status = !todo.is_empty() && std::io::stderr().is_terminal();
        if status {
            eprint!("looking up package names...");
        }
        let found: Vec<Option<String>> = std::thread::scope(|s| {
            let handles: Vec<_> = todo.iter().map(|(m, c)| s.spawn(move || lookup(m, c))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or(None)).collect()
        });
        if status {
            eprint!("\r\x1b[2K"); // clear the status line
        }
        for ((m, c), pkg) in todo.iter().zip(found) {
            self.cache.put(&m.name, c, pkg, now);
        }
        self.cache.save();

        commands
            .iter()
            .map(|c| {
                let hints = self
                    .managers
                    .iter()
                    .filter_map(|m| {
                        let looked_up = self.cache.get(&m.name, c, now).flatten();
                        // Guessing "python -m json.tool" or "git undo" as a package is nonsense.
                        let guessable = !c.contains(' ');
                        let (package, guessed) = match looked_up {
                            Some(p) => (p, false),
                            None if guessable => (c.to_string(), true),
                            None => return None,
                        };
                        Some(Hint {
                            manager: m.name.clone(),
                            install: m.install.replace("{package}", &package),
                            try_cmd: m.try_cmd.as_ref().map(|t| t.replace("{package}", &package)),
                            guessed,
                        })
                    })
                    .collect();
                (c.to_string(), hints)
            })
            .collect()
    }

    /// Packages whose name or description matches `query`, per manager, in
    /// parallel. Cached like lookups.
    pub fn search(&mut self, query: &str) -> Vec<(String, Found)> {
        let now = now();
        let todo: Vec<&Manager> = self
            .managers
            .iter()
            .filter(|m| m.search.is_some() && self.cache.get_search(&m.name, query, now).is_none())
            .collect();
        let status = !todo.is_empty() && std::io::stderr().is_terminal();
        if status {
            let names: Vec<&str> = todo.iter().map(|m| m.name.as_str()).collect();
            eprint!("searching {} packages...", names.join(", "));
        }
        let results: Vec<Option<Vec<Found>>> = std::thread::scope(|s| {
            let handles: Vec<_> = todo.iter().map(|m| s.spawn(|| search(m, query))).collect();
            handles.into_iter().map(|h| h.join().unwrap_or(None)).collect()
        });
        if status {
            eprint!("\r\x1b[2K");
        }
        for (m, found) in todo.iter().zip(results) {
            // Don't cache timeouts; the next run may be faster (warm nix eval cache).
            if let Some(found) = found {
                self.cache.put_search(&m.name, query, found, now);
            }
        }
        self.cache.save();
        self.managers
            .iter()
            .filter_map(|m| self.cache.get_search(&m.name, query, now).map(|f| (m, f)))
            .flat_map(|(m, found)| found.iter().map(|f| (m.name.clone(), f.clone())).collect::<Vec<_>>())
            .collect()
    }

    /// Install commands for packages found by `search` (name already known).
    pub fn install_hints(&self, found: &[(String, Found)]) -> HashMap<String, Vec<Hint>> {
        let mut hints: HashMap<String, Vec<Hint>> = HashMap::new();
        for (manager, f) in found {
            let Some(m) = self.managers.iter().find(|m| &m.name == manager) else { continue };
            hints.entry(f.name.clone()).or_default().push(Hint {
                manager: m.name.clone(),
                install: m.install.replace("{package}", &f.name),
                try_cmd: m.try_cmd.as_ref().map(|t| t.replace("{package}", &f.name)),
                guessed: false,
            });
        }
        hints
    }
}

fn search(m: &Manager, query: &str) -> Option<Vec<Found>> {
    let s = m.search.as_ref()?;
    let re = Regex::new(&s.pattern).ok()?;
    let argv: Vec<String> = s.command.iter().map(|a| a.replace("{query}", query)).collect();
    let timeout = s.timeout.map_or(LOOKUP_TIMEOUT, Duration::from_secs);
    let output = run(&argv, timeout)?;
    Some(parse_search(&re, &output))
}

fn parse_search(re: &Regex, output: &str) -> Vec<Found> {
    let mut found: Vec<Found> = Vec::new();
    for c in re.captures_iter(output) {
        let Some(name) = c.get(1).map(|m| m.as_str().to_string()) else { continue };
        if !found.iter().any(|f| f.name == name) {
            let description = c.get(2).map(|d| d.as_str().trim().to_string()).filter(|d| !d.is_empty());
            found.push(Found { name, description });
        }
    }
    found
}

/// Ask the package manager which package provides `command`.
fn lookup(m: &Manager, command: &str) -> Option<String> {
    // Subcommands like `git undo` are usually separate binaries (`git-undo`).
    let binary = command.replace(' ', "-");
    m.lookup.iter().find_map(|l| {
        let re = Regex::new(&l.pattern).ok()?;
        let argv: Vec<String> = l.command.iter().map(|a| a.replace("{command}", &binary)).collect();
        let output = run(&argv, LOOKUP_TIMEOUT)?;
        output.lines().find_map(|line| re.captures(line.trim()).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()))
    })
}

/// Run a command (no shell) and return stdout + stderr, or None on timeout.
fn run(argv: &[String], timeout: Duration) -> Option<String> {
    let (program, args) = argv.split_first()?;
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let (mut out, mut err) = (child.stdout.take()?, child.stderr.take()?);
    // Read in threads so a chatty command can't fill the pipe and block.
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = out.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = err.read_to_string(&mut s);
        s
    });
    let start = Instant::now();
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if start.elapsed() > timeout {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Some(format!("{}\n{}", out_t.join().ok()?, err_t.join().ok()?))
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Lookup results (including "not found") per manager and command.
#[derive(Default, Serialize, Deserialize)]
struct Cache {
    entries: HashMap<String, (Option<String>, u64)>,
    #[serde(default)]
    searches: HashMap<String, (Vec<Found>, u64)>,
}

impl Cache {
    fn file() -> Option<PathBuf> {
        crate::index::cache_dir().ok().map(|d| d.join("packages.json"))
    }

    fn load() -> Self {
        Self::file()
            .and_then(|f| std::fs::read(f).ok())
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self) {
        if let (Some(f), Ok(bytes)) = (Self::file(), serde_json::to_vec(self)) {
            let _ = std::fs::write(f, bytes);
        }
    }

    /// `Some(None)` = cached "no package found"; `None` = not cached.
    fn get(&self, manager: &str, command: &str, now: u64) -> Option<Option<String>> {
        let (pkg, at) = self.entries.get(&format!("{manager}:{command}"))?;
        (now.saturating_sub(*at) < CACHE_TTL.as_secs()).then(|| pkg.clone())
    }

    fn get_search(&self, manager: &str, query: &str, now: u64) -> Option<&Vec<Found>> {
        let (found, at) = self.searches.get(&format!("{manager}:{query}"))?;
        (now.saturating_sub(*at) < SEARCH_TTL.as_secs()).then_some(found)
    }

    fn put_search(&mut self, manager: &str, query: &str, found: Vec<Found>, now: u64) {
        self.searches.retain(|_, (_, at)| now.saturating_sub(*at) < SEARCH_TTL.as_secs());
        self.searches.insert(format!("{manager}:{query}"), (found, now));
    }

    fn put(&mut self, manager: &str, command: &str, pkg: Option<String>, now: u64) {
        self.entries.insert(format!("{manager}:{command}"), (pkg, now));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manager(name: &str) -> Manager {
        let config: Config = toml::from_str(BUILTIN).unwrap();
        config.managers.into_iter().find(|m| m.name == name).unwrap()
    }

    /// Apply a manager's lookup patterns to sample output, as `lookup` does.
    fn parse(name: &str, output: &str) -> Option<String> {
        manager(name).lookup.iter().find_map(|l| {
            let re = Regex::new(&l.pattern).unwrap();
            output.lines().find_map(|line| re.captures(line.trim()).and_then(|c| c.get(1)).map(|m| m.as_str().to_string()))
        })
    }

    #[test]
    fn builtin_table_parses() {
        let config: Config = toml::from_str(BUILTIN).unwrap();
        assert!(config.managers.len() >= 8);
        for m in &config.managers {
            assert!(m.install.contains("{package}"), "{}: install needs {{package}}", m.name);
            for l in &m.lookup {
                Regex::new(&l.pattern).unwrap();
                assert!(l.command.iter().any(|a| a.contains("{command}")), "{}: lookup needs {{command}}", m.name);
            }
        }
    }

    #[test]
    fn parses_real_outputs() {
        assert_eq!(parse("nix", "ripgrep.out").unwrap(), "ripgrep");
        assert_eq!(parse("nix", "glow").unwrap(), "glow");
        assert_eq!(parse("nix", "python312Packages.httpie.out").unwrap(), "python312Packages.httpie");
        assert_eq!(parse("pacman", "extra/ripgrep").unwrap(), "ripgrep");
        assert_eq!(
            parse("apt", "Command 'rg' not found, but can be installed with:\n\nsudo apt install ripgrep").unwrap(),
            "ripgrep"
        );
        assert_eq!(parse("apt", "ripgrep: /usr/bin/rg").unwrap(), "ripgrep");
        assert_eq!(
            parse("dnf", "ripgrep-14.1.0-1.fc40.x86_64 : Line-oriented search tool").unwrap(),
            "ripgrep"
        );
        assert_eq!(
            parse("zypper", "S | Name    | Summary | Type\n--+---------+---------+--------\n  | ripgrep | A search tool | package").unwrap(),
            "ripgrep"
        );
        assert_eq!(parse("apk", "ripgrep-14.1.1-r0").unwrap(), "ripgrep");
        assert_eq!(parse("xbps", "ripgrep-14.1.0_1\t/usr/bin/rg").unwrap(), "ripgrep");
        assert_eq!(parse("brew", "ripgrep").unwrap(), "ripgrep");
    }

    fn search_output(name: &str, output: &str) -> Vec<(String, Option<String>)> {
        let re = Regex::new(&manager(name).search.unwrap().pattern).unwrap();
        parse_search(&re, output).into_iter().map(|f| (f.name, f.description)).collect()
    }

    #[test]
    fn parses_search_outputs() {
        let want = |d: &str| vec![("tealdeer".to_string(), Some(d.to_string()))];
        let d = "A very fast implementation of tldr in Rust";
        assert_eq!(
            search_output("nix", &format!("* legacyPackages.x86_64-linux.tealdeer (1.7.1)\n  {d}\n\n")),
            want(d)
        );
        assert_eq!(search_output("pacman", &format!("extra/tealdeer 1.7.1-1 [installed]\n    {d}\n")), want(d));
        assert_eq!(search_output("apt", &format!("tealdeer - {d}\n")), want(d));
        assert_eq!(search_output("dnf", &format!("Matched fields: name\n tealdeer.x86_64\t{d}\n")), want(d));
        assert_eq!(search_output("dnf", &format!("tealdeer.x86_64 : {d}\n")), want(d));
        assert_eq!(
            search_output("zypper", &format!("S | Name     | Summary | Type\n--+---+---+---\n  | tealdeer | {d} | package\n")),
            want(d)
        );
        assert_eq!(search_output("apk", &format!("tealdeer-1.7.1-r0 - {d}\n")), want(d));
        assert_eq!(search_output("xbps", &format!("[-] tealdeer-1.7.1_1   {d}\n")), want(d));
        assert_eq!(search_output("brew", "==> Formulae\ntealdeer\n"), vec![("tealdeer".to_string(), None)]);
    }

    #[test]
    fn ignores_noise() {
        assert_eq!(parse("pacman", ""), None);
        assert_eq!(parse("dnf", "Error: No matches found."), None);
    }
}
