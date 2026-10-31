//! Rule helpers for a host's `authorize` in `PermissionMode::Auto` (docs/12 §5.2).
//!
//! A usability guard rail: it catches common destructive or outward-reaching commands so they
//! are asked about again. It is not a security boundary: aliases, variables, encodings and
//! other interpreters get past it, so a host never relaxes `Deny` or its own checks on it.

use std::mem;
use std::path::{Component, Path, PathBuf};

/// Why a command or script should be asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Risk {
    /// Stable snake_case: `delete`, `disk`, `git_destructive`, `permissions`, `publish`,
    /// `global_install`, `elevation`, `download_exec`, `home_path`, `process_spawn`,
    /// `network`, `file_delete`.
    pub reason: &'static str,
}

/// Language of a script checked by [`script_risk`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScriptLanguage {
    /// POSIX shell or bash.
    Shell,
    /// PowerShell.
    PowerShell,
    /// Python.
    Python,
    /// JavaScript run by a Node-like runtime.
    JavaScript,
}

const fn risk(reason: &'static str) -> Option<Risk> {
    Some(Risk { reason })
}

/// Words that run the next word as the program.
const WRAPPERS: &[&str] = &[
    "env", "time", "nohup", "command", "exec", "xargs", "nice", "builtin",
];
const ELEVATION: &[&str] = &["sudo", "doas", "runas", "su"];
const DELETE: &[&str] = &[
    "rm",
    "rmdir",
    "del",
    "erase",
    "rd",
    "ri",
    "remove-item",
    "shred",
    "unlink",
];
const DISK: &[&str] = &[
    "dd", "format", "diskpart", "fdisk", "parted", "shutdown", "reboot", "poweroff", "halt",
];
const DOWNLOAD: &[&str] = &[
    "curl",
    "wget",
    "iwr",
    "irm",
    "invoke-webrequest",
    "invoke-restmethod",
];
const SHELLS: &[&str] = &[
    "sh",
    "bash",
    "zsh",
    "dash",
    "pwsh",
    "powershell",
    "iex",
    "invoke-expression",
];
/// Lower-case markers of the user's home directory inside a word.
const HOME: &[&str] = &[
    "~/",
    "~\\",
    "$home",
    "${home}",
    "%userprofile%",
    "$env:userprofile",
];

type Rule = (&'static str, &'static str);
const PYTHON: &[Rule] = &[
    ("subprocess", "process_spawn"),
    ("os.system", "process_spawn"),
    ("os.popen", "process_spawn"),
    ("os.remove", "file_delete"),
    ("os.unlink", "file_delete"),
    ("shutil.rmtree", "file_delete"),
    ("socket", "network"),
    ("urllib", "network"),
    ("requests", "network"),
    ("http.client", "network"),
];
const JAVASCRIPT: &[Rule] = &[
    ("child_process", "process_spawn"),
    ("fs.rm", "file_delete"),
    ("fs.rmdir", "file_delete"),
    ("fs.unlink", "file_delete"),
    ("rmSync", "file_delete"),
    ("rmdirSync", "file_delete"),
    ("unlinkSync", "file_delete"),
    ("fetch(", "network"),
    ("http.", "network"),
    ("https.", "network"),
    ("net.", "network"),
];

/// The first risk in a shell or PowerShell command line, if any.
///
/// Splits into commands at `&&`, `||`, `;`, `&` and newlines, and into pipeline stages at `|`
/// (and at `(`, `)` and backticks, so substitutions are checked as well). Each stage's program
/// is the first word after `VAR=x` prefixes and wrappers such as `env`, `nohup` or `xargs`,
/// without its path, `.exe` and case. Quoted text is one word, so `echo "rm -rf /"` is fine.
#[must_use]
pub fn shell_risk(command: &str) -> Option<Risk> {
    parse(command)
        .iter()
        .find_map(|stages| segment_risk(stages))
}

/// The first risk in a script: shell and PowerShell line by line with [`shell_risk`]; Python
/// and JavaScript by the process, file-deletion and network APIs they name.
#[must_use]
pub fn script_risk(language: ScriptLanguage, source: &str) -> Option<Risk> {
    let rules = match language {
        ScriptLanguage::Shell | ScriptLanguage::PowerShell => {
            return source.lines().find_map(shell_risk);
        }
        ScriptLanguage::Python => PYTHON,
        ScriptLanguage::JavaScript => JAVASCRIPT,
    };
    rules
        .iter()
        .find(|(needle, _)| contains_token(source, needle))
        .map(|&(_, reason)| Risk { reason })
}

/// Whether `path` lies inside one of `roots`, after resolving `.` and `..` lexically only.
/// Relative paths and roots never match. Symbolic links are not resolved: a host that needs
/// that canonicalizes both sides where it does I/O, before the synchronous `authorize`.
#[must_use]
pub fn within(path: &Path, roots: &[PathBuf]) -> bool {
    let Some(path) = normalize(path) else {
        return false;
    };
    roots.iter().filter_map(|root| normalize(root)).any(|root| {
        let mut inside = path.components();
        root.components()
            .all(|part| inside.next().is_some_and(|other| same(part, other)))
    })
}

fn normalize(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => out.push(part),
            Component::CurDir => {}
            // `..` at the root stays at the root, as the OS resolves it.
            Component::ParentDir => {
                if out.parent().is_some() {
                    out.pop();
                }
            }
        }
    }
    Some(out)
}

/// Windows paths compare without case.
fn same(a: Component<'_>, b: Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}

/// Commands of pipeline stages of words.
type Segment = Vec<Vec<String>>;

#[derive(Default)]
struct Parser {
    segments: Vec<Segment>,
    stages: Segment,
    words: Vec<String>,
    word: String,
    /// The word had quotes, so an empty `""` still counts.
    quoted: bool,
}

impl Parser {
    fn end_word(&mut self) {
        if !self.word.is_empty() || self.quoted {
            self.words.push(mem::take(&mut self.word));
        }
        self.quoted = false;
    }
    fn end_stage(&mut self) {
        self.end_word();
        if !self.words.is_empty() {
            self.stages.push(mem::take(&mut self.words));
        }
    }
    fn end_segment(&mut self) {
        self.end_stage();
        if !self.stages.is_empty() {
            self.segments.push(mem::take(&mut self.stages));
        }
    }
}

fn parse(command: &str) -> Vec<Segment> {
    let mut parser = Parser::default();
    let mut quote = None;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(open) = quote {
            if c == open {
                quote = None;
            } else if c == '\\' && open == '"' {
                match chars.next_if(|n| matches!(n, '"' | '\\' | '$' | '`')) {
                    Some(n) => parser.word.push(n),
                    None => parser.word.push('\\'),
                }
            } else {
                parser.word.push(c);
            }
            continue;
        }
        match c {
            '\'' | '"' => {
                quote = Some(c);
                parser.quoted = true;
            }
            // Escapes only before what the shell would otherwise interpret, so Windows paths
            // such as `C:\tools\rm.exe` keep their separators.
            '\\' => {
                if chars.next_if_eq(&'\n').is_none() {
                    match chars.next_if(|n| " \t'\"\\;&|()`".contains(*n)) {
                        Some(n) => parser.word.push(n),
                        None => parser.word.push('\\'),
                    }
                }
            }
            '#' if parser.word.is_empty() && !parser.quoted => {
                while chars.next_if(|n| *n != '\n').is_some() {}
            }
            ' ' | '\t' | '\r' => parser.end_word(),
            '\n' | ';' => parser.end_segment(),
            // `2>&1` and `&>file` are redirections, not separators.
            '&' if parser.word.ends_with(['>', '<']) || chars.peek() == Some(&'>') => {
                parser.word.push('&');
            }
            '&' => {
                chars.next_if_eq(&'&');
                parser.end_segment();
            }
            '|' => {
                if chars.next_if_eq(&'|').is_some() {
                    parser.end_segment();
                } else {
                    chars.next_if_eq(&'&');
                    parser.end_stage();
                }
            }
            '(' | ')' | '`' => parser.end_stage(),
            _ => parser.word.push(c),
        }
    }
    parser.end_segment();
    parser.segments
}

/// The start of one stage.
enum Head<'a> {
    /// The program's normalized name and its arguments.
    Program(String, &'a [String]),
    /// An elevating wrapper such as `sudo`.
    Elevated,
    /// Only assignments and wrappers.
    Empty,
}

fn head(words: &[String]) -> Head<'_> {
    let mut rest = words;
    let mut wrapped = false;
    while let Some((first, tail)) = rest.split_first() {
        let name = name(first);
        if ELEVATION.contains(&name.as_str()) {
            return Head::Elevated;
        }
        let option = first.starts_with('-') || first.bytes().all(|b| b.is_ascii_digit());
        if assignment(first) || wrapped && option {
            rest = tail;
        } else if WRAPPERS.contains(&name.as_str()) {
            wrapped = true;
            rest = tail;
        } else {
            return Head::Program(name, tail);
        }
    }
    Head::Empty
}

fn segment_risk(stages: &[Vec<String>]) -> Option<Risk> {
    if let Some(found) = stages.iter().find_map(|words| stage_risk(words)) {
        return Some(found);
    }
    let names: Vec<String> = stages
        .iter()
        .filter_map(|words| match head(words) {
            Head::Program(name, _) => Some(name),
            Head::Elevated | Head::Empty => None,
        })
        .collect();
    let has = |list: &[&str]| names.iter().any(|name| list.contains(&name.as_str()));
    if has(DOWNLOAD) && has(SHELLS) {
        return risk("download_exec");
    }
    None
}

fn stage_risk(words: &[String]) -> Option<Risk> {
    let found = match head(words) {
        Head::Program(name, args) => program_risk(&name, args),
        Head::Elevated => return risk("elevation"),
        Head::Empty => None,
    };
    found.or_else(|| {
        words.iter().any(|word| home(word)).then_some(Risk {
            reason: "home_path",
        })
    })
}

fn program_risk(name: &str, args: &[String]) -> Option<Risk> {
    let any = |test: &dyn Fn(&str) -> bool| args.iter().any(|arg| test(arg));
    match name {
        _ if DELETE.contains(&name) => risk("delete"),
        "find" if find_deletes(args) => risk("delete"),
        _ if DISK.contains(&name) || name.starts_with("mkfs") => risk("disk"),
        "git" => git_risk(args),
        "chmod" | "chown" | "chgrp" if any(&|a| a == "--recursive" || short(a, 'R')) => {
            risk("permissions")
        }
        "icacls" | "takeown" | "killall" | "pkill" | "stop-process" => risk("permissions"),
        "kill" if kills(args) => risk("permissions"),
        "taskkill" if any(&|a| a.eq_ignore_ascii_case("/f") || a == "-f") => risk("permissions"),
        "start-process" if any(&|a| a.eq_ignore_ascii_case("runas")) => risk("elevation"),
        "npm" | "pnpm" | "yarn" | "bun" | "cargo" => package_risk(name, args),
        "pip" | "pip3" => pip_risk(args),
        _ if name.starts_with("python")
            && args.first().is_some_and(|a| a == "-m")
            && args.get(1).is_some_and(|a| a == "pip") =>
        {
            pip_risk(args.get(2..).unwrap_or_default())
        }
        "sh" | "bash" | "zsh" | "dash" | "cmd" | "pwsh" | "powershell" => inline_risk(args),
        _ => None,
    }
}

/// `find … -delete` or `find … -exec rm …`.
fn find_deletes(args: &[String]) -> bool {
    args.iter().enumerate().any(|(at, arg)| {
        arg == "-delete"
            || matches!(arg.as_str(), "-exec" | "-execdir" | "-ok" | "-okdir")
                && args
                    .get(at + 1)
                    .is_some_and(|next| DELETE.contains(&name(next).as_str()))
    })
}

/// `kill -9`, `kill -KILL`, `kill -s KILL` and the like.
fn kills(args: &[String]) -> bool {
    let signal = |s: &str| matches!(s.to_ascii_uppercase().as_str(), "9" | "KILL" | "SIGKILL");
    args.iter()
        .any(|arg| arg.strip_prefix('-').is_some_and(signal))
        || args
            .windows(2)
            .any(|pair| matches!(pair, [flag, value] if flag == "-s" && signal(value)))
}

fn git_risk(args: &[String]) -> Option<Risk> {
    let mut rest = args;
    // Global options before the subcommand; `-C` and `-c` take a value.
    while let Some((first, tail)) = rest.split_first() {
        if !first.starts_with('-') {
            break;
        }
        rest = if matches!(first.as_str(), "-C" | "-c") {
            tail.get(1..).unwrap_or_default()
        } else {
            tail
        };
    }
    let (sub, args) = rest.split_first()?;
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let cluster = |flag: char| args.iter().any(|arg| short(arg, flag));
    let destructive = match sub.as_str() {
        "push" => {
            has("--force")
                || cluster('f')
                || args
                    .iter()
                    .any(|arg| arg.starts_with("--force-with-lease") || arg.starts_with('+'))
        }
        "reset" => has("--hard"),
        "clean" => has("--force") || cluster('f'),
        "checkout" => has("--") || has("."),
        "restore" => !(has("--staged") || cluster('S')) || has("--worktree") || cluster('W'),
        "branch" => cluster('D') || has("--delete") && (has("--force") || cluster('f')),
        _ => false,
    };
    destructive.then_some(Risk {
        reason: "git_destructive",
    })
}

fn package_risk(manager: &str, args: &[String]) -> Option<Risk> {
    let sub = args.iter().find(|arg| !arg.starts_with('-'))?.as_str();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    if sub == "publish" && !has("--dry-run") {
        return risk("publish");
    }
    let global = match manager {
        "cargo" => sub == "install",
        "yarn" if sub == "global" => true,
        _ => {
            matches!(sub, "add" | "install" | "i")
                && (has("-g") || has("--global") || has("--location=global"))
        }
    };
    global.then_some(Risk {
        reason: "global_install",
    })
}

fn pip_risk(args: &[String]) -> Option<Risk> {
    let install = args
        .iter()
        .find(|arg| !arg.starts_with('-'))
        .is_some_and(|sub| sub == "install");
    (install && args.iter().any(|arg| arg == "--user" || arg == "--system")).then_some(Risk {
        reason: "global_install",
    })
}

/// `bash -c "…"`, `cmd /c …`, `pwsh -Command …`: the inline command is checked as well, and a
/// download inside it (`sh -c "$(curl …)"`) counts as running what was downloaded.
fn inline_risk(args: &[String]) -> Option<Risk> {
    let at = args.iter().position(|arg| {
        matches!(
            arg.to_ascii_lowercase().as_str(),
            "-c" | "-lc" | "/c" | "/k" | "-command"
        )
    })?;
    let inline = args.get(at + 1..)?.join(" ");
    shell_risk(&inline).or_else(|| {
        parse(&inline)
            .iter()
            .flatten()
            .any(|words| {
                matches!(head(words), Head::Program(name, _) if DOWNLOAD.contains(&name.as_str()))
            })
            .then_some(Risk {
                reason: "download_exec",
            })
    })
}

/// Program name without directories, a Windows executable extension or case.
fn name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word);
    let mut lower = base.to_lowercase();
    for extension in [".exe", ".cmd", ".bat", ".com"] {
        if let Some(stem) = lower.strip_suffix(extension) {
            lower = stem.to_owned();
            break;
        }
    }
    lower
}

/// `NAME=value` before the program.
fn assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(key, _)| {
        key.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
    })
}

/// A short option cluster such as `-fd` that contains `flag`.
fn short(arg: &str, flag: char) -> bool {
    arg.strip_prefix('-').is_some_and(|cluster| {
        !cluster.starts_with('-')
            && cluster.chars().all(|c| c.is_ascii_alphanumeric())
            && cluster.contains(flag)
    })
}

fn identifier(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn home(word: &str) -> bool {
    let lower = word.to_lowercase();
    HOME.iter().any(|marker| {
        lower.match_indices(marker).any(|(at, _)| {
            // `$HOMEBREW_PREFIX` is another variable.
            !marker.ends_with(identifier) || !lower[at + marker.len()..].starts_with(identifier)
        })
    })
}

/// `needle` in `source` not glued to a longer identifier (`websocket` is not `socket`).
fn contains_token(source: &str, needle: &str) -> bool {
    source.match_indices(needle).any(|(at, _)| {
        let glued_before = needle.starts_with(identifier)
            && source[..at].chars().next_back().is_some_and(identifier);
        let glued_after = needle.ends_with(identifier)
            && source[at + needle.len()..]
                .chars()
                .next()
                .is_some_and(identifier);
        !glued_before && !glued_after
    })
}

#[cfg(test)]
mod tests;
