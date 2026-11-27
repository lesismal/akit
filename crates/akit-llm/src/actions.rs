//! A node's fixed actions (fit's `llm/node/actions.go`). A node never runs a script a caller
//! gives: every management operation is one of these, its command built here, its parameters
//! checked against an anchored pattern (and for control characters and spaces) and only ever
//! passed as argv elements of their own, never through a shell. The few operations that need a
//! shell (vendors' install scripts) run a script constant of this program with no parameter.

use std::sync::OnceLock;

use akio_regex::Regex;

use crate::exec::{Command, look_path};
use crate::json::{self, Obj};

/// An Ollama model reference: `name[:tag]`, with a registry or namespace path; its first
/// character a letter or digit, so that a value is never taken for an option.
pub const PATTERN_OLLAMA_MODEL: &str = r"^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$";
/// A count of lines, 1 to 9999.
pub const PATTERN_LINES: &str = r"^[1-9][0-9]{0,3}$";

/// Every parameter's value is this long at most, whatever its pattern.
const MAX_PARAM_LEN: usize = 256;

/// Ollama's official Linux install (it sets up the ollama service).
pub const SCRIPT_OLLAMA_INSTALL: &str = "set -e
command -v curl >/dev/null 2>&1 || { echo \"curl is required\"; exit 1; }
curl -fsSL https://ollama.com/install.sh | sh
ollama --version || true
";

/// The NVIDIA Container Toolkit installed from NVIDIA's repository, and Docker configured and
/// restarted when it is there.
pub const SCRIPT_NVIDIA_CONTAINER_TOOLKIT: &str = "set -e
if command -v apt-get >/dev/null 2>&1; then
  export DEBIAN_FRONTEND=noninteractive
  apt-get update
  apt-get install -y curl gpg
  curl -fsSL https://nvidia.github.io/libnvidia-container/gpgkey | gpg --yes --dearmor -o /usr/share/keyrings/nvidia-container-toolkit-keyring.gpg
  curl -fsSL https://nvidia.github.io/libnvidia-container/stable/deb/nvidia-container-toolkit.list \\
    | sed 's#deb https://#deb [signed-by=/usr/share/keyrings/nvidia-container-toolkit-keyring.gpg] https://#g' \\
    > /etc/apt/sources.list.d/nvidia-container-toolkit.list
  apt-get update
  apt-get install -y nvidia-container-toolkit
elif command -v dnf >/dev/null 2>&1 || command -v yum >/dev/null 2>&1; then
  PM=dnf; command -v dnf >/dev/null 2>&1 || PM=yum
  curl -fsSL https://nvidia.github.io/libnvidia-container/stable/rpm/nvidia-container-toolkit.repo \\
    > /etc/yum.repos.d/nvidia-container-toolkit.repo
  $PM install -y nvidia-container-toolkit
else
  echo \"Unsupported distribution: apt-get, dnf or yum is required\"; exit 1
fi
if command -v docker >/dev/null 2>&1; then
  nvidia-ctk runtime configure --runtime=docker
  systemctl restart docker || true
fi
nvidia-ctk --version
";

/// A parameter of an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Param {
    /// Its key.
    pub key: &'static str,
    /// Its label.
    pub label: &'static str,
    /// Its label in Chinese.
    pub label_zh: &'static str,
    /// Its anchored pattern.
    pub pattern: &'static str,
    /// An example.
    pub example: &'static str,
}

/// How an action's command is built from its checked parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Build {
    /// This argv.
    Argv(&'static [&'static str]),
    /// This argv, as root.
    RootArgv(&'static [&'static str]),
    /// `sh -c` with this constant script, as root.
    RootScript(&'static str),
    /// `ollama <verb> <model>`.
    OllamaModel(&'static str),
    /// `journalctl -u ollama -n <lines> --no-pager`, as root.
    OllamaLogs,
    /// Ollama's install: the script on Linux, Homebrew on macOS.
    OllamaInstall,
}

/// A fixed action (fit's `model.Action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Action {
    /// Its ID.
    pub id: &'static str,
    /// Its title.
    pub title: &'static str,
    /// Its title in Chinese.
    pub title_zh: &'static str,
    /// A note.
    pub note: &'static str,
    /// The note in Chinese.
    pub note_zh: &'static str,
    /// Its parameters.
    pub params: &'static [Param],
    /// The systems it runs on (Go's `GOOS`); empty: every one.
    pub os: &'static [&'static str],
    /// Its command.
    pub build: Build,
}

const LINUX: &[&str] = &["linux"];
const LINUX_DARWIN: &[&str] = &["linux", "darwin"];
const LINUX_WINDOWS: &[&str] = &["linux", "windows"];

const MODEL_PARAM: Param = Param {
    key: "model",
    label: "Model",
    label_zh: "模型",
    pattern: PATTERN_OLLAMA_MODEL,
    example: "qwen3:8b",
};

const fn act(
    id: &'static str,
    title: &'static str,
    title_zh: &'static str,
    build: Build,
) -> Action {
    Action {
        id,
        title,
        title_zh,
        note: "",
        note_zh: "",
        params: &[],
        os: &[],
        build,
    }
}

/// Every action, as fit lists them.
pub const ACTIONS: &[Action] = &[
    Action {
        os: LINUX_DARWIN,
        note: "Linux: the official install script from ollama.com (sets up the ollama service). macOS: brew install ollama.",
        note_zh: "Linux：使用 ollama.com 官方安装脚本（会配置 ollama 服务）。macOS：brew install ollama。",
        ..act(
            "ollama_install",
            "Install Ollama",
            "安装 Ollama",
            Build::OllamaInstall,
        )
    },
    act(
        "ollama_list",
        "List Ollama models",
        "列出 Ollama 模型",
        Build::Argv(&["ollama", "list"]),
    ),
    act(
        "ollama_ps",
        "Show loaded Ollama models",
        "查看已加载的 Ollama 模型",
        Build::Argv(&["ollama", "ps"]),
    ),
    Action {
        params: &[MODEL_PARAM],
        note: "Downloads the model from the Ollama library",
        note_zh: "从 Ollama 模型库下载模型",
        ..act(
            "ollama_pull",
            "Pull an Ollama model",
            "拉取 Ollama 模型",
            Build::OllamaModel("pull"),
        )
    },
    Action {
        params: &[MODEL_PARAM],
        ..act(
            "ollama_show",
            "Show an Ollama model",
            "查看 Ollama 模型",
            Build::OllamaModel("show"),
        )
    },
    Action {
        params: &[MODEL_PARAM],
        ..act(
            "ollama_rm",
            "Remove an Ollama model",
            "删除 Ollama 模型",
            Build::OllamaModel("rm"),
        )
    },
    Action {
        os: LINUX,
        ..act(
            "ollama_status",
            "Ollama service status",
            "Ollama 服务状态",
            Build::Argv(&["systemctl", "status", "ollama", "--no-pager"]),
        )
    },
    Action {
        os: LINUX,
        ..act(
            "ollama_restart",
            "Restart the Ollama service",
            "重启 Ollama 服务",
            Build::RootArgv(&["systemctl", "restart", "ollama"]),
        )
    },
    Action {
        os: LINUX,
        params: &[Param {
            key: "lines",
            label: "Lines",
            label_zh: "行数",
            pattern: PATTERN_LINES,
            example: "200",
        }],
        ..act(
            "ollama_logs",
            "Ollama service logs",
            "Ollama 服务日志",
            Build::OllamaLogs,
        )
    },
    Action {
        os: LINUX_WINDOWS,
        ..act(
            "nvidia_smi",
            "Show nvidia-smi",
            "查看 nvidia-smi",
            Build::Argv(&["nvidia-smi"]),
        )
    },
    Action {
        os: LINUX,
        note: "Adds NVIDIA's repository (apt / dnf / yum), installs the toolkit and configures Docker when present",
        note_zh: "添加 NVIDIA 软件源（apt / dnf / yum），安装工具包；存在 Docker 时为其配置并重启",
        ..act(
            "nvidia_container_toolkit_install",
            "Install NVIDIA Container Toolkit",
            "安装 NVIDIA Container Toolkit",
            Build::RootScript(SCRIPT_NVIDIA_CONTAINER_TOOLKIT),
        )
    },
];

/// This system as Go's `GOOS` names it.
#[must_use]
pub fn goos() -> &'static str {
    match std::env::consts::OS {
        "macos" => "darwin",
        o => o,
    }
}

/// This machine as Go's `GOARCH` names it.
#[must_use]
pub fn goarch() -> &'static str {
    match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        "x86" => "386",
        a => a,
    }
}

fn supported(os: &[&str]) -> bool {
    os.is_empty() || os.contains(&goos())
}

impl Action {
    /// As JSON (fit's `model.Action`).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = Obj::new()
            .str("id", self.id)
            .str("title", self.title)
            .str("title_zh", self.title_zh)
            .str_omit("note", self.note)
            .str_omit("note_zh", self.note_zh);
        if !self.params.is_empty() {
            o = o.raw(
                "params",
                &json::array(self.params.iter().map(|p| {
                    Obj::new()
                        .str("key", p.key)
                        .str("label", p.label)
                        .str("label_zh", p.label_zh)
                        .str("pattern", p.pattern)
                        .str_omit("example", p.example)
                        .end()
                })),
            );
        }
        if !self.os.is_empty() {
            let os: Vec<String> = self.os.iter().map(|s| (*s).to_owned()).collect();
            o = o.strs("os", &os);
        }
        o.end()
    }
}

/// The actions this system runs, as JSON (`/node/action/list`).
#[must_use]
pub fn list_json() -> String {
    json::array(
        ACTIONS
            .iter()
            .filter(|a| supported(a.os))
            .map(Action::to_json),
    )
}

fn regex(pattern: &str) -> Option<&'static Regex> {
    static RES: OnceLock<Vec<(&'static str, Regex)>> = OnceLock::new();
    let res = RES.get_or_init(|| {
        let mut v: Vec<(&'static str, Regex)> = Vec::new();
        for a in ACTIONS {
            for p in a.params {
                // An unanchored pattern, or one that does not compile, matches nothing.
                if p.pattern.starts_with('^')
                    && p.pattern.ends_with('$')
                    && !v.iter().any(|(k, _)| *k == p.pattern)
                {
                    if let Ok(r) = Regex::new(p.pattern) {
                        v.push((p.pattern, r));
                    }
                }
            }
        }
        v
    });
    res.iter().find(|(k, _)| *k == pattern).map(|(_, r)| r)
}

/// Checks `v` for `p`: within the length, no control character or space, matching its pattern.
///
/// # Errors
///
/// Why it does not.
pub fn check_param(p: &Param, v: &str) -> Result<(), String> {
    if v.len() > MAX_PARAM_LEN
        || v.chars().any(|c| c.is_control() || c.is_whitespace())
        || !regex(p.pattern).is_some_and(|r| r.is_match(v))
    {
        return Err(format!("invalid {} {v:?}: must match {}", p.key, p.pattern));
    }
    Ok(())
}

/// Checks `params` against action `id` and returns its command and a job's title (fit's
/// `BuildAction`): every parameter declared is required, an unknown one refused, each value
/// checked by [`check_param`]. The caller applies [`Command::sudo`] and runs it as a job.
///
/// # Errors
///
/// An unknown action, one this system does not run, a parameter missing, unknown or invalid.
pub fn build(id: &str, params: &[(String, String)]) -> Result<(Command, String), String> {
    let Some(a) = ACTIONS.iter().find(|a| a.id == id) else {
        return Err(format!("unknown action {id:?}"));
    };
    if !supported(a.os) {
        return Err(format!("action {id} is not supported on {}", goos()));
    }
    for (k, _) in params {
        if !a.params.iter().any(|p| p.key == k) {
            return Err(format!("action {id}: unknown parameter {k:?}"));
        }
    }
    let mut title = a.title.to_owned();
    let get = |k: &str| {
        params
            .iter()
            .find(|(pk, _)| pk == k)
            .map_or("", |(_, v)| v.as_str())
    };
    for p in a.params {
        let v = get(p.key);
        if v.is_empty() {
            return Err(format!("action {id}: missing parameter {:?}", p.key));
        }
        check_param(p, v).map_err(|e| format!("action {id}: {e}"))?;
        title.push(' ');
        title.push_str(v);
    }
    let owned = |a: &[&str]| a.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
    let c = match a.build {
        Build::Argv(v) => Command {
            argv: owned(v),
            ..Command::default()
        },
        Build::RootArgv(v) => Command {
            argv: owned(v),
            root: true,
            ..Command::default()
        },
        Build::RootScript(s) => Command {
            argv: owned(&["sh", "-c", s]),
            root: true,
            ..Command::default()
        },
        Build::OllamaModel(verb) => Command {
            argv: owned(&["ollama", verb, get("model")]),
            ..Command::default()
        },
        Build::OllamaLogs => Command {
            argv: owned(&[
                "journalctl",
                "-u",
                "ollama",
                "-n",
                get("lines"),
                "--no-pager",
            ]),
            root: true,
            ..Command::default()
        },
        Build::OllamaInstall => ollama_install()?,
    };
    Ok((c, title))
}

/// Ollama's install: the official script on Linux, Homebrew on macOS (never as root, which
/// Homebrew refuses).
fn ollama_install() -> Result<Command, String> {
    if goos() == "linux" {
        return Ok(Command {
            argv: vec!["sh".into(), "-c".into(), SCRIPT_OLLAMA_INSTALL.into()],
            root: true,
            ..Command::default()
        });
    }
    let brew = look_path("brew")
        .or_else(|| look_path("/opt/homebrew/bin/brew"))
        .or_else(|| look_path("/usr/local/bin/brew"))
        .ok_or_else(|| {
            "homebrew not found: install it from https://brew.sh, or download Ollama from https://ollama.com/download"
                .to_owned()
        })?;
    Ok(Command {
        argv: vec![
            brew.to_string_lossy().into_owned(),
            "install".into(),
            "ollama".into(),
        ],
        ..Command::default()
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::indexing_slicing)]
    use super::*;

    fn p(k: &str, v: &str) -> Vec<(String, String)> {
        vec![(k.to_owned(), v.to_owned())]
    }

    /// fit's `TestBuildActionValidation`.
    #[test]
    fn validation() {
        let (c, title) = build(
            "ollama_pull",
            &p("model", "hf.co/bartowski/Qwen3-8B-GGUF:Q4_K_M"),
        )
        .unwrap();
        assert_eq!(
            c.argv,
            ["ollama", "pull", "hf.co/bartowski/Qwen3-8B-GGUF:Q4_K_M"]
        );
        assert!(title.contains("Qwen3"));
        let long = "a".repeat(200);
        for v in [
            "x; rm -rf /",
            "$(id)",
            "`id`",
            "-flag",
            "--insecure",
            "x\nid",
            "x\n",
            "x y",
            "x|id",
            "x&&id",
            "x>y",
            "",
            ".hidden",
            "a\0b",
            "qwen x",
            long.as_str(),
        ] {
            for id in ["ollama_pull", "ollama_rm", "ollama_show"] {
                assert!(build(id, &p("model", v)).is_err(), "{id} model={v:?}");
            }
        }
        assert!(build("ollama_pull", &[]).is_err(), "a parameter missing");
        assert!(
            build("ollama_list", &p("script", "id")).is_err(),
            "an unknown parameter"
        );
        assert!(build("run_script", &[]).is_err(), "an unknown action");
        if goos() == "linux" {
            for v in ["0", "10000", "1e3", "-5", "5;id"] {
                assert!(build("ollama_logs", &p("lines", v)).is_err(), "lines={v:?}");
            }
        } else {
            assert!(build("ollama_status", &[]).is_err(), "a Linux action");
        }
    }

    /// fit's `TestActionCatalog`: every action complete, its examples valid, a shell only with a
    /// constant script and no parameter.
    #[test]
    fn catalog() {
        let mut seen = Vec::new();
        for a in ACTIONS {
            assert!(!a.id.is_empty() && !seen.contains(&a.id), "{}", a.id);
            assert!(!a.title.is_empty() && !a.title_zh.is_empty());
            seen.push(a.id);
            let mut params = Vec::new();
            for p in a.params {
                assert!(!p.label.is_empty() && !p.label_zh.is_empty() && !p.example.is_empty());
                check_param(p, p.example).unwrap();
                params.push((p.key.to_owned(), p.example.to_owned()));
            }
            if !supported(a.os) || a.id == "ollama_install" {
                continue;
            }
            let (c, _) = build(a.id, &params).unwrap();
            assert!(!c.argv.is_empty());
            if c.argv[0] == "sh" {
                assert!(a.params.is_empty());
                assert!(
                    [SCRIPT_OLLAMA_INSTALL, SCRIPT_NVIDIA_CONTAINER_TOOLKIT]
                        .contains(&c.argv[2].as_str())
                );
            }
        }
        let list = list_json();
        assert!(
            list.starts_with(r#"[{"id":"ollama_install""#)
                || goos() != "linux" && goos() != "darwin"
        );
        assert!(list.contains(r#""params":[{"key":"model","label":"Model","label_zh":"模型","pattern":"^[a-zA-Z0-9][a-zA-Z0-9._:/-]{0,127}$","example":"qwen3:8b"}]"#));
    }
}
