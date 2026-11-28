//! fit's `skills_test.go`, `skills_page_test.go`, `skills_load_test.go` and
//! `skills_chat_test.go`, ported, on the assistant's shared stand-ins (`crate::testkit`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::too_many_lines
)]

use std::sync::Arc;

use akit_config::value::{Value, parse};

use super::*;
use crate::Store;
use crate::audit::Filter;
use crate::standin;
use crate::testkit::*;

const GOOD: &str = "\u{feff}---\r\nname: my-skill
title: My skill
title_zh: 我的技能
description: Does what I need.
description_zh: 做我需要的事。
tools: [config_get, config_propose]
model: m/other-model
inputs:
  - name: host
    description: The host.
    required: true
  - name: port
    required: false
examples:
  - Do it for example.com.
---

Read the routes, then do it.
";

/// fit's `TestParseSkill`: a skill's Markdown is read whole; front matter that is not a skill's
/// is refused with the line it is about.
#[test]
fn parse_skill() {
    let sk = super::parse(GOOD.as_bytes()).unwrap();
    assert!(
        sk.name == "my-skill"
            && sk.title == "My skill"
            && sk.title_zh == "我的技能"
            && sk.description_zh == "做我需要的事。"
            && sk.tools == ["config_get", "config_propose"]
            && sk.model == "m/other-model"
            && sk.inputs.len() == 2
            && sk.inputs[0].required
            && !sk.inputs[1].required
            && sk.inputs[0].description == "The host."
            && sk.examples.len() == 1
            && sk.prompt == "Read the routes, then do it."
            && !sk.source.contains('\r')
            && sk.input("port").is_some()
            && sk.input("x").is_none(),
        "{sk:?}"
    );
    // Lists left empty, and tools left out.
    let sk =
        super::parse(b"---\nname: a\ntitle: A\ndescription: d\ninputs:\nexamples:\ntools:\n---\np")
            .unwrap();
    assert!(sk.tools.is_empty() && sk.inputs.is_empty());

    let front = |lines: &[&str]| format!("---\n{}\n---\nThe prompt.\n", lines.join("\n"));
    let with = |extra: &[&str]| {
        let mut l = vec!["name: a", "title: A", "description: d"];
        l.extend_from_slice(extra);
        front(&l)
    };
    let tags = format!("tools: [{}t]", "t,".repeat(33));
    let zh = format!("title_zh: {}", "长".repeat(50));
    let many = format!("inputs: [{}{{name: b}}]", "{name: a},".repeat(8));
    for (src, line, msg) in [
        (
            "name: a\n---\np".to_owned(),
            1,
            "starts with its front matter",
        ),
        ("---\nname: a\n".to_owned(), 1, "no closing line"),
        (front(&[]), 1, "the front matter is empty"),
        (front(&["# only a comment"]), 1, "the front matter is empty"),
        (front(&["- a", "- b"]), 2, "not a mapping"),
        (
            front(&["name: a", "title: [A", "description: d"]),
            3,
            "not YAML",
        ),
        (front(&["name: a", "title: A: B"]), 3, "not YAML"),
        (front(&["name: a", "\ttitle: x"]), 3, "not YAML"),
        (with(&["colour: red"]), 5, "\"colour\" is not a key"),
        (with(&["[a]: b"]), 5, "not a name"),
        (with(&["title: B"]), 5, "title is given twice"),
        (front(&["name: a", "title: A"]), 1, "has no description"),
        (
            front(&["name: a", "title:", "description: d"]),
            1,
            "has no title",
        ),
        (
            front(&["name: Not-OK", "title: A", "description: d"]),
            2,
            "is not a skill's name",
        ),
        (
            front(&["name: [a]", "title: A", "description: d"]),
            2,
            "name is not a text",
        ),
        (with(&[zh.as_str()]), 5, "title_zh is 150 bytes"),
        (
            with(&["description_zh: |", "  one", "  two"]),
            5,
            "is one line",
        ),
        (with(&["model: has space"]), 5, "is not a model"),
        (with(&["tools: config_get"]), 5, "tools is not a list"),
        (
            with(&["tools: [Config]"]),
            5,
            "\"Config\" is not a tool's name",
        ),
        (
            with(&["tools:", "  - config_get", "  - config_get"]),
            7,
            "named twice",
        ),
        (with(&[tags.as_str()]), 5, "it has at most 32"),
        (with(&["tools: [[a]]"]), 5, "item is not a text"),
        (with(&["examples: ['']"]), 5, "an empty item"),
        (with(&["inputs: a"]), 5, "inputs is not a list"),
        (with(&["inputs: [a]"]), 5, "an input is a mapping"),
        (
            with(&["inputs:", "  - name: A"]),
            6,
            "is not an input's name",
        ),
        (
            with(&["inputs:", "  - name: a", "    colour: red"]),
            7,
            "\"colour\" is not a key of an input",
        ),
        (
            with(&["inputs:", "  - name: a", "    name: b"]),
            7,
            "name is given twice",
        ),
        (
            with(&["inputs:", "  - name: a", "    required: yes"]),
            7,
            "required is true or false",
        ),
        (
            with(&["inputs:", "  - description: x"]),
            6,
            "an input has no name",
        ),
        (
            with(&["inputs:", "  - name: a", "  - name: a"]),
            7,
            "the input a is given twice",
        ),
        (with(&[many.as_str()]), 5, "at most 8"),
        (
            with(&["inputs:", "  - name: a", "    description: [x]"]),
            7,
            "not a text",
        ),
        (
            front(&["name: a", "title: &x A", "description: *x"]),
            4,
            "aliases are not taken",
        ),
        (
            "---\nname: a\ntitle: A\ndescription: d\n---\n\n  \n".to_owned(),
            5,
            "no prompt",
        ),
    ] {
        match super::parse(src.as_bytes()) {
            Err(e) => assert!(
                e.line == line && e.msg.contains(msg),
                "{src:?}: {e} (line {}), want line {line} {msg:?}",
                e.line
            ),
            Ok(sk) => panic!("{src:?}: {sk:?}, want line {line} {msg:?}"),
        }
    }
    let e = super::parse(b"---\nname: a\n\xff\n---\np").unwrap_err();
    assert!(e.line == 3 && e.msg.contains("not UTF-8"), "{e}");
    let e = super::parse("x".repeat(MAX_SKILL_BYTES + 1).as_bytes()).unwrap_err();
    assert!(e.to_string().contains("at most 32768") && !e.to_string().starts_with("line"));
    assert_eq!(err(3, "m").to_string(), "line 3: m");
}

/// fit's `TestSkillPrompt`: the current skill's part of the system prompt names the inputs not
/// given, the required ones as such.
#[test]
fn skill_prompt() {
    let sk = super::parse(GOOD.as_bytes()).unwrap();
    let sys = crate::prompt::system(&crate::prompt::Parts {
        level: Level::Read,
        skill: Some(crate::prompt::SkillPart {
            skill: &sk,
            inputs: r#"{"port":"80"}"#.to_owned(),
            tools: vec!["config_get".to_owned()],
            not: vec!["config_propose (needs the level propose)".to_owned()],
        }),
        ..crate::prompt::Parts::default()
    });
    for want in [
        "The current skill is my-skill (My skill)",
        "<skill name=\"my-skill\">\nRead the routes, then do it.\n</skill>",
        "- host (required): The host.",
        r#""port":"80""#,
        "The tools you may call for it: config_get.",
        "config_propose (needs the level propose)",
    ] {
        assert!(sys.contains(want), "no {want:?} in {sys}");
    }
    assert!(!sys.contains("- port"), "{sys}");
}

/// The tools of fit that akit's assistant has not yet, by the iteration that brings them: a
/// built-in skill may name them, and its runs wait for them.
const TOOLS_TO_COME: &[(&str, &str)] = &[
    ("page_get", "I167"),
    ("page_edit", "I167"),
    ("page_revert", "I167"),
    ("media_library_search", "I168"),
    ("media_item_get", "I168"),
    ("media_scan_propose", "I168"),
    ("media_file_propose", "I168"),
    ("media_session_end_propose", "I168"),
    ("playback_sessions", "I168"),
    ("channels_status", "I168"),
    ("cluster_members", "I169"),
    ("cluster_metrics", "I169"),
    ("cluster_log_tail", "I169"),
    ("cluster_certs", "I169"),
    ("cluster_rollouts", "I169"),
    ("cluster_templates", "I169"),
    ("cluster_op_propose", "I169"),
    ("rollout_propose", "I169"),
    ("drift_fix_propose", "I169"),
    ("sync_pipelines", "the sync track"),
    ("sync_dry_run", "the sync track"),
    ("sync_test", "the sync track"),
    ("sync_dead_letters", "the sync track"),
];

/// fit's `TestBuiltinSkills`: fit's built-in skills are embedded, in its order, each with its
/// words in English and Chinese, examples, and only tools the registry has or a later iteration
/// brings.
#[test]
fn builtin_skills() {
    let list = builtin();
    assert_eq!(list.len(), BUILTIN.len());
    let f = start("", vec![text("-")]);
    for (i, sk) in list.iter().enumerate() {
        assert!(
            sk.name == BUILTIN[i].0
                && !sk.title_zh.is_empty()
                && !sk.description_zh.is_empty()
                && !sk.examples.is_empty()
                && !sk.tools.is_empty()
                && sk.prompt.len() >= 400
                && !sk.prompt.contains("fit's")
                && !sk.prompt.contains("this fit"),
            "{sk:?}"
        );
        for name in &sk.tools {
            assert!(
                name != "ask_user" && name != "skill_load",
                "{} names {name}",
                sk.name
            );
            assert!(
                f.a.registry().get(name).is_some() || TOOLS_TO_COME.iter().any(|(t, _)| t == name),
                "{} names the tool {name}",
                sk.name
            );
        }
    }
}

const OPS: &str = "---
name: ops-check
title: Ops check
title_zh: 运维检查
description: Checks what the ops team checks every morning.
tools: [status_get, config_propose, no_such_tool]
---
Read the status and say whether the listeners are bound.
";

fn skill_json(src: &str) -> String {
    Obj::new().str("source", src).end()
}

fn send(f: &Fx, method: &str, path: &str, body: &str) -> (u16, String) {
    f.send(method, &format!("/api/m/ai{path}"), body)
}

fn listing(f: &Fx) -> Value {
    let (status, body) = f.get("/api/skills");
    assert_eq!(status, 200, "{body}");
    parse(&body).unwrap()
}

fn skills_of(v: &Value) -> &[Value] {
    match v.get("skills") {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

fn last_user(r: &standin::Req) -> String {
    let v = r.json().unwrap();
    match v.get("messages") {
        Some(Value::Array(ms)) => ms
            .iter()
            .rev()
            .find(|m| json::str_of(m, "role") == "user")
            .map(|m| json::str_of(m, "content").to_owned())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// The tools a request to the model offers, sorted.
fn offered(r: &standin::Req) -> Vec<String> {
    let v = r.json().unwrap();
    let mut out: Vec<String> = match v.get("tools") {
        Some(Value::Array(ts)) => ts
            .iter()
            .filter_map(|t| {
                t.get("function")
                    .map(|f| json::str_of(f, "name").to_owned())
            })
            .collect(),
        _ => Vec::new(),
    };
    out.sort();
    out
}

/// The tools' results a request to the model sends back.
fn tool_results(r: &standin::Req) -> Vec<String> {
    let v = r.json().unwrap();
    match v.get("messages") {
        Some(Value::Array(ms)) => ms
            .iter()
            .filter(|m| json::str_of(m, "role") == "tool")
            .map(|m| json::str_of(m, "content").to_owned())
            .collect(),
        _ => Vec::new(),
    }
}

/// The skills the page lists: the built-ins but a manager's, and `added` more.
fn shown(added: usize) -> usize {
    BUILTIN.len() - CLUSTER_SKILLS.len() + added
}

/// fit's `TestUserSkills`: a skill added on the Skills page is listed, read whole, changed,
/// enabled and disabled; runs by `/name`; holds across a restart with the built-ins disabled;
/// and is refused, with its line, when its front matter is wrong. Only a user whose level is
/// apply changes skills.
#[test]
fn user_skills() {
    let f = start(
        r#", "default_level": "apply", "users": [{"username": "bob", "level": "propose"}]"#,
        vec![text("All bound.")],
    );
    let l = listing(&f);
    let first = &skills_of(&l)[0];
    assert!(
        skills_of(&l).len() == shown(0)
            && b(&l, "can_change")
            && b(&l, "kept")
            && n(&l, "max") == MAX_USER_SKILLS as u64
            && s(&l, "level") == "apply"
            && b(first, "builtin")
            && b(first, "enabled")
            && first.get("prompt").is_none()
            && first.get("source").is_none(),
        "{}",
        json::compact(&l)
    );

    // Refused with its line.
    assert_eq!(f.post("/api/skills", "not json").0, 400);
    let (status, body) = f.post(
        "/api/skills",
        &skill_json("---\nname: x\ntitle: X\ndescription: d\ncolour: red\n---\np"),
    );
    assert!(
        status == 400
            && body.contains(r#""line":5"#)
            && body.contains(r#"line 5: \"colour\" is not a key"#),
        "{status} {body}"
    );
    let (status, body) = f.post(
        "/api/skills",
        &skill_json(&OPS.replace("ops-check", "diagnose")),
    );
    assert!(
        status == 409 && body.contains("a built-in skill is named diagnose"),
        "{status} {body}"
    );

    // Added.
    let (status, body) = f.post("/api/skills", &skill_json(OPS));
    let v = parse(&body).unwrap();
    let state = arr(&v, "tool_state").to_vec();
    assert!(
        status == 200
            && s(&v, "name") == "ops-check"
            && !b(&v, "builtin")
            && b(&v, "enabled")
            && s(&v, "by") == "ada"
            && s(&v, "source") == OPS
            && !s(&v, "prompt").is_empty()
            && state.len() == 3
            && b(&state[0], "ok")
            && b(&state[1], "ok")
            && !b(&state[2], "ok")
            && s(&state[2], "why") == "this akit has no such tool",
        "{status} {body}"
    );
    let (status, body) = f.post("/api/skills", &skill_json(OPS));
    assert!(
        status == 409 && body.contains("named ops-check already"),
        "{status} {body}"
    );
    assert!(
        f.log_text()
            .contains("console: \"ada\" added the assistant's skill ops-check"),
        "{}",
        f.log_text()
    );
    let (status, body) = f.get("/api/skills/ops-check");
    assert!(
        status == 200 && body.contains(r#""source":"---\nname: ops-check"#),
        "{status} {body}"
    );
    assert_eq!(f.get("/api/skills/nope").0, 404);
    let (status, body) = f.get("/api/skills/explain");
    assert!(
        status == 200
            && body.contains(r#""builtin":true"#)
            && body.contains(r#""prompt":"You explain akit"#),
        "{status} {body}"
    );

    // It runs, by /name.
    let c = f.chat(r#"{"message": "/ops-check is everything bound?"}"#);
    assert!(
        c.end().is_some() && s(c.of("start")[0], "skill") == "ops-check",
        "{:?}",
        c.events
    );
    let sys = system(&f.model_calls()[0]);
    assert!(
        sys.contains("Read the status and say whether the listeners are bound.")
            && sys.contains("no_such_tool (this akit has no such tool)"),
        "{sys}"
    );

    // Changed, and renamed; a built-in is neither changed nor deleted.
    let changed = OPS.replace("every morning", "every evening");
    let (status, body) = send(&f, "PUT", "/api/skills/ops-check", &skill_json(&changed));
    assert!(
        status == 200 && body.contains("every evening"),
        "{status} {body}"
    );
    let renamed = changed.replace("name: ops-check", "name: ops-daily");
    let (status, body) = send(&f, "PUT", "/api/skills/ops-check", &skill_json(&renamed));
    assert!(
        status == 200 && body.contains(r#""name":"ops-daily""#),
        "{status} {body}"
    );
    for (method, path, body, want, msg) in [
        (
            "PUT",
            "/api/skills/explain",
            skill_json(OPS),
            409,
            "a built-in skill is not changed",
        ),
        (
            "PUT",
            "/api/skills/nope",
            skill_json(OPS),
            404,
            "no skill nope was added",
        ),
        (
            "PUT",
            "/api/skills/ops-daily",
            skill_json(&OPS.replace("ops-check", "explain")),
            409,
            "a built-in skill is named explain",
        ),
        (
            "PUT",
            "/api/skills/ops-daily",
            skill_json("x"),
            400,
            "line 1",
        ),
        (
            "DELETE",
            "/api/skills/explain",
            String::new(),
            409,
            "disable it",
        ),
        (
            "DELETE",
            "/api/skills/nope",
            String::new(),
            404,
            "no skill nope was added",
        ),
        (
            "POST",
            "/api/skills/nope/enable",
            "{}".to_owned(),
            404,
            "no skill nope",
        ),
        (
            "PATCH",
            "/api/skills/explain",
            "{}".to_owned(),
            405,
            "GET, PUT or DELETE",
        ),
    ] {
        let (status, out) = send(&f, method, path, &body);
        assert!(
            status == want && out.contains(msg),
            "{method} {path}: {status} {out}"
        );
    }

    // Enabled and disabled: a disabled skill is out of the catalog and not run.
    for p in [
        "/api/skills/ops-daily/disable",
        "/api/skills/explain/disable",
        "/api/skills/diagnose/disable",
        "/api/skills/diagnose/enable",
    ] {
        let (status, out) = f.post(p, "{}");
        assert_eq!(status, 200, "{p}: {out}");
    }
    let (status, _) = f.post("/api/chat", r#"{"message": "x", "skill": "ops-daily"}"#);
    assert_eq!(status, 404);
    let cat = f.a.catalog();
    assert!(
        !cat.iter()
            .any(|e| e.name == "explain" || e.name == "ops-daily")
    );
    assert_eq!(cat.len(), shown(0) - 1);
    assert_eq!(f.post("/api/skills/ops-daily/enable", "{}").0, 200);

    // bob, whose level is propose, reads and runs them, and changes none.
    let bob = f.as_user("bob");
    let l = listing(&bob);
    assert!(!b(&l, "can_change") && skills_of(&l).len() == shown(1));
    for (method, path, body) in [
        ("POST", "/api/skills", skill_json(OPS)),
        ("PUT", "/api/skills/ops-daily", skill_json(OPS)),
        ("DELETE", "/api/skills/ops-daily", String::new()),
        ("POST", "/api/skills/ops-daily/disable", "{}".to_owned()),
    ] {
        let (status, out) = send(&bob, method, path, &body);
        assert!(
            status == 403 && out.contains("needs the level apply"),
            "{method} {path}: {status} {out}"
        );
    }
    let c = bob.chat(r#"{"message": "/ops-daily go"}"#);
    assert!(c.end().is_some() && s(c.of("start")[0], "skill") == "ops-daily");

    // A restart: the skill and the built-in disabled hold.
    let store = Arc::clone(&f.store);
    let g = start_with(r#", "default_level": "apply""#, vec![text("-")], |o| {
        o.store = Some(store as Arc<dyn Store>);
    });
    let l = listing(&g);
    let by = |name: &str| {
        skills_of(&l)
            .iter()
            .find(|v| s(v, "name") == name)
            .cloned()
            .unwrap()
    };
    let daily = by("ops-daily");
    assert!(
        !b(&daily, "builtin")
            && b(&daily, "enabled")
            && s(&daily, "by") == "ada"
            && n(&daily, "at") > 0
            && !b(&by("explain"), "enabled")
            && b(&by("diagnose"), "enabled"),
        "after a restart: {}",
        json::compact(&l)
    );
    assert!(g.a.enabled_skill("ops-daily").is_some() && g.a.enabled_skill("explain").is_none());

    // Deleted.
    let (status, body) = send(&g, "DELETE", "/api/skills/ops-daily", "");
    assert!(
        status == 200 && body.contains(r#""deleted":"ops-daily""#),
        "{status} {body}"
    );
    assert!(g.a.enabled_skill("ops-daily").is_none());
}

/// fit's `TestUserSkillsBounds`: the skills added are bounded in number; a store that fails
/// keeps what was; a store whose skills no longer load leaves them out and says so; without a
/// store they last until a restart.
#[test]
fn user_skills_bounds() {
    let f = start(r#", "default_level": "apply""#, vec![text("ok")]);
    for i in 0..MAX_USER_SKILLS {
        let src = format!("---\nname: s{i}\ntitle: S\ndescription: d\n---\np\n");
        let (status, body) = f.post("/api/skills", &skill_json(&src));
        assert_eq!(status, 200, "{i}: {body}");
    }
    let (status, body) = f.post(
        "/api/skills",
        &skill_json("---\nname: one-more\ntitle: S\ndescription: d\n---\np\n"),
    );
    assert!(
        status == 409 && body.contains("as many as may be"),
        "{status} {body}"
    );

    // A store that refuses keeps what was.
    let ro = Arc::new(Mem {
        fail: true,
        ..Mem::default()
    });
    let h = start_with(r#", "default_level": "apply""#, vec![text("-")], |o| {
        o.store = Some(ro as Arc<dyn Store>);
    });
    let (status, body) = h.post("/api/skills", &skill_json(OPS));
    assert!(
        status == 500 && body.contains("read-only"),
        "{status} {body}"
    );
    assert!(
        h.a.enabled_skill("ops-check").is_none(),
        "a change the store refused was used"
    );

    // The store's section: not JSON, or a skill that no longer loads.
    let st = Arc::new(Mem::default());
    st.set_section(SECTION, b"not json").unwrap();
    let g = start_with("", vec![text("-")], |o| {
        o.store = Some(Arc::clone(&st) as Arc<dyn Store>);
    });
    assert!(
        g.log_text()
            .contains("the skills kept in the store do not load")
            && g.a.enabled_skills().len() == shown(0),
        "{}",
        g.log_text()
    );
    let kept = format!(
        r#"{{"user":[{{"source":"broken"}},{{"source":{}}},{{"source":{},"disabled":true}}],"disabled":["explain"]}}"#,
        quoted(&OPS.replace("ops-check", "diagnose")),
        quoted(OPS)
    );
    st.set_section(SECTION, kept.as_bytes()).unwrap();
    let g = start_with("", vec![text("-")], |o| {
        o.store = Some(Arc::clone(&st) as Arc<dyn Store>);
    });
    assert!(
        g.log_text()
            .matches("a skill kept in the store does not load")
            .count()
            == 2
            && g.a.enabled_skill("ops-check").is_none()
            && g.a.enabled_skill("explain").is_none(),
        "{}",
        g.log_text()
    );
    assert!(matches!(
        g.a.skill_snap().find("ops-check"),
        Some((_, false, false))
    ));

    // Without a store.
    let h = start_with(r#", "default_level": "apply""#, vec![text("-")], |o| {
        o.store = None;
    });
    assert_eq!(h.post("/api/skills", &skill_json(OPS)).0, 200);
    assert!(!b(&listing(&h), "kept") && h.a.enabled_skill("ops-check").is_some());
}

/// fit's `TestSkillPickedAndLoaded`: a request that names no skill carries the catalog only; the
/// model picks a skill from it and loads it with `skill_load`, whose result is the skill's whole
/// prompt; the calls after it carry the prompt in the system prompt, not the catalog, and offer
/// the skill's tools only, so a tool it does not name cannot be called; the conversation runs
/// the skill from then on, until the person clears it.
#[test]
fn skill_picked_and_loaded() {
    let f = start(
        r#", "default_level": "propose""#,
        vec![
            calls(&[("skill_load", r#"{"name": "diagnose"}"#)]),
            calls(&[
                ("config_propose", r#"{"summary": "x", "members": {}}"#),
                ("upstream_health", "{}"),
            ]),
            text("The backend 10.0.0.7 is probed down."),
        ],
    );
    let c = f.chat(r#"{"message": "why does /api answer 502?"}"#);
    assert_eq!(
        c.end().map(|e| s(e, "reason")),
        Some("answer"),
        "{:?}",
        c.err()
    );
    let reqs = f.model_calls();
    let diagnose = f.a.enabled_skill("diagnose").unwrap();
    let sys0 = system(&reqs[0]);
    assert!(
        sys0.contains("Skills you may load with skill_load")
            && sys0.contains(&format!("- diagnose: {}", diagnose.description))
            && sys0.contains("- route-builder: ")
            && !sys0.contains(&diagnose.prompt[..80])
            && !sys0.contains("The current skill"),
        "the first call: {sys0}"
    );
    let got = offered(&reqs[0]);
    assert!(
        got.iter().any(|t| t == "skill_load") && got.iter().any(|t| t == "config_propose"),
        "{got:?}"
    );
    let ev = c.of("skill");
    assert!(
        ev.len() == 1 && s(ev[0], "skill") == "diagnose",
        "{:?}",
        c.events
    );
    let res = c.of("tool_result");
    let loaded = s(res[0], "content");
    assert!(
        !b(res[0], "error")
            && loaded.contains("<skill name=\"diagnose\">")
            && loaded.contains(&diagnose.prompt)
            && loaded.contains("Your tools now: ")
            && loaded.contains("access_log_sample (data.send_access_log is off)"),
        "skill_load: {loaded}"
    );
    assert!(
        b(res[1], "error")
            && s(res[1], "content").contains("there is no tool config_propose you may call")
            && !b(res[2], "error"),
        "{res:?}"
    );
    let sys1 = system(&reqs[1]);
    assert!(
        !sys1.contains("Skills you may load")
            && sys1.contains("The current skill is diagnose (Diagnose)")
            && sys1.contains(&diagnose.prompt),
        "the second call: {sys1}"
    );
    let got = offered(&reqs[1]);
    assert!(
        !got.iter()
            .any(|t| t == "skill_load" || t == "config_propose")
            && got.iter().any(|t| t == "upstream_health")
            && got.iter().any(|t| t == "ask_user"),
        "{got:?}"
    );
    // The prompt is sent once: the result that loaded it is a note.
    let tr = tool_results(&reqs[1]);
    assert!(
        tr.len() == 1
            && !tr[0].contains(&diagnose.prompt)
            && tr[0].contains("a skill was loaded here"),
        "{tr:?}"
    );
    let conv = c.conv();
    assert_eq!(f.a.audit_entries(&Filter::default()).0[0].skill, "diagnose");

    // The next message runs the skill still; the list says so.
    f.sc.set(vec![text("ok")]);
    let c = f.chat(&format!(
        r#"{{"conversation": "{conv}", "message": "and now?"}}"#
    ));
    assert_eq!(s(c.of("start")[0], "skill"), "diagnose");
    let reqs = f.model_calls();
    assert!(system(reqs.last().unwrap()).contains("The current skill is diagnose"));
    assert!(
        f.get("/api/conversations")
            .1
            .contains(r#""skill":"diagnose""#)
    );
    // Cleared, the catalog is back.
    let c = f.chat(&format!(
        r#"{{"conversation": "{conv}", "message": "something else", "skill": ""}}"#
    ));
    assert_eq!(s(c.of("start")[0], "skill"), "");
    let reqs = f.model_calls();
    let sys = system(reqs.last().unwrap());
    assert!(
        !sys.contains("The current skill") && sys.contains("Skills you may load"),
        "{sys}"
    );
    f.no_secrets();
}

/// fit's `TestSkillAboveLevel`: a skill naming a tool above the user's level does not offer it,
/// says so in the prompt, and a call of it fails.
#[test]
fn skill_above_level() {
    let f = start(
        "",
        vec![
            calls(&[("config_propose", r#"{"summary": "x", "members": {}}"#)]),
            text("I cannot propose it; a user whose level is propose can."),
        ],
    );
    let c = f.chat(r#"{"message": "/route-builder answer /healthz with ok"}"#);
    assert!(c.end().is_some(), "{:?}", c.err());
    let reqs = f.model_calls();
    let sys = system(&reqs[0]);
    assert!(
        sys.contains("It names tools you may not call: config_propose (needs the level propose)")
            && sys.contains("Inputs the skill takes that were not given")
            && sys.contains("- host: The host name"),
        "{sys}"
    );
    let got = offered(&reqs[0]);
    assert!(
        !got.iter().any(|t| t == "config_propose") && got.iter().any(|t| t == "config_get"),
        "{got:?}"
    );
    let r = c.of("tool_result");
    assert!(
        r.len() == 1
            && b(r[0], "error")
            && s(r[0], "content").contains("there is no tool config_propose"),
        "{r:?}"
    );
    assert_eq!(last_user(&reqs[0]), "answer /healthz with ok");
}

/// fit's `TestSkillBodies`: what a chat's body says of its skill is checked; inputs reach the
/// prompt as data, redacted; a skill disabled since is dropped from its conversation.
#[test]
fn skill_bodies() {
    let f = start(r#", "default_level": "apply""#, vec![text("ok")]);
    let long = format!(
        r#"{{"message": "x", "skill": "explain", "inputs": {{"path": "{}"}}}}"#,
        "p".repeat(MAX_INPUT_VALUE + 1)
    );
    for (body, want, msg) in [
        (
            r#"{"retry": true, "conversation": "x", "skill": "explain"}"#,
            400,
            "a skill is set with a message",
        ),
        (
            r#"{"message": "x", "inputs": {"a": "b"}}"#,
            400,
            "inputs go with the skill",
        ),
        (
            r#"{"message": "x", "skill": "", "inputs": {"a": "b"}}"#,
            400,
            "inputs go with the skill",
        ),
        (
            r#"{"message": "x", "skill": "nope"}"#,
            404,
            "no enabled skill is named nope",
        ),
        (
            r#"{"message": "x", "skill": "explain", "inputs": {"colour": "red"}}"#,
            400,
            "takes no input colour",
        ),
        (long.as_str(), 400, "at most 2048 bytes"),
        (
            r#"{"message": "x", "skill": "explain", "inputs": {"a":"","b":"","c":"","d":"","e":"","f":"","g":"","h":"","i":""}}"#,
            400,
            "at most 8 inputs",
        ),
        (
            r#"{"message": "/explain"}"#,
            400,
            "after /explain, say what",
        ),
        (
            "{\"message\": \"/explain   \\n \"}",
            400,
            "after /explain, say what",
        ),
    ] {
        let (status, out) = f.post("/api/chat", body);
        assert!(
            status == want && out.contains(msg),
            "{body}: {status} {out}"
        );
    }
    // A /word that is no skill is the person's words, as written.
    let c = f.chat(r#"{"message": "/etc/hosts: what is it?"}"#);
    assert!(c.end().is_some());
    assert_eq!(last_user(&f.model_calls()[0]), "/etc/hosts: what is it?");
    // /name on its own line.
    let c = f.chat("{\"message\": \"/explain\\nthe listeners\"}");
    assert_eq!(s(c.of("start")[0], "skill"), "explain");
    assert_eq!(last_user(f.model_calls().last().unwrap()), "the listeners");
    // Inputs, as data and redacted.
    let c = f.chat(&format!(
        r#"{{"message": "this one", "skill": "explain", "inputs": {{"path": "routes", "error": "Authorization: Bearer {SECRET}"}}}}"#
    ));
    assert!(c.end().is_some(), "{:?}", c.err());
    let conv = c.conv();
    let sys = system(f.model_calls().last().unwrap());
    assert!(
        sys.contains("<skill_inputs>\n{")
            && sys.contains(r#""path":"routes""#)
            && !sys.contains("Inputs the skill takes that were not given"),
        "{sys}"
    );
    f.no_secrets();

    // Disabled since, the skill is dropped from the conversation.
    assert_eq!(f.post("/api/skills/explain/disable", "{}").0, 200);
    let c = f.chat(&format!(
        r#"{{"conversation": "{conv}", "message": "and again"}}"#
    ));
    assert_eq!(s(c.of("start")[0], "skill"), "");
    let (_, one) = f.get(&format!("/api/conversations/{conv}"));
    assert!(
        !one.contains(r#""skill":"explain""#) && one.contains("and again"),
        "{one}"
    );
    // A disabled skill is not run by /name: the words go as written.
    let c = f.chat(r#"{"message": "/explain this"}"#);
    assert_eq!(s(c.of("start")[0], "skill"), "");
}

/// fit's `TestSkillModel`: a skill's model is the one its chats call: a provider's model, a
/// model of the default provider, or the default's with a name it does not know; a model that
/// calls no tools gets none, and the prompt says so.
#[test]
fn skill_model() {
    let f = start_with(r#", "default_level": "apply""#, vec![text("ok")], |o| {
        o.settings
            .providers
            .get_mut(0)
            .unwrap()
            .models
            .push(akit_config::assistant::Model {
                name: "plain".into(),
                context: 0,
                tools: Some(false),
            });
    });
    for (name, model) in [
        ("by-provider", "local/plain"),
        ("by-model", "other-model"),
        ("unknown-provider", "nope/x"),
    ] {
        let src = format!(
            "---\nname: {name}\ntitle: T\ndescription: d\ntools: [config_get]\nmodel: {model}\n---\nDo it.\n"
        );
        let (status, out) = f.post("/api/skills", &skill_json(&src));
        assert_eq!(status, 200, "{out}");
    }
    for (name, want) in [
        ("by-provider", "plain"),
        ("by-model", "other-model"),
        ("unknown-provider", "nope/x"),
    ] {
        let c = f.chat(&format!(r#"{{"message": "/{name} go"}}"#));
        assert_eq!(s(c.of("start")[0], "model"), want, "{name}: {:?}", c.events);
        let reqs = f.model_calls();
        let got = reqs.last().unwrap().json().unwrap();
        assert_eq!(s(&got, "model"), want);
    }
    let r = &f.model_calls()[0];
    let sys = system(r);
    assert!(
        offered(r).is_empty()
            && sys.contains("You may call no tools for it")
            && sys.contains("config_get (the model calls no tools)"),
        "{sys}"
    );
}

/// fit's `TestSkillLoadEdges`: `skill_load`'s own failures, and the catalog by name past its
/// share.
#[test]
fn skill_load_edges() {
    let f = start("", vec![text("-")]);
    let a = &f.a;
    let settings = a.settings();
    let run = |args: &str| {
        let call = Call {
            a,
            args: parse(args).unwrap(),
            user: "ada",
            settings: &settings,
            conv: "",
            expired: None,
            chat: None,
        };
        run_load(&call)
    };
    assert!(
        run(r#"{"name": "nope"}"#)
            .unwrap_err()
            .contains("no enabled skill is named")
    );
    assert!(
        run(r#"{"name": 1}"#)
            .unwrap_err()
            .contains("not what the tool takes")
    );
    // Outside a chat, as over MCP, it is the prompt alone.
    let out = run(r#"{"name": "explain"}"#).unwrap();
    assert!(
        out.contains("<skill name=\"explain\">") && !out.contains("Your tools now"),
        "{out}"
    );
    let sk = Skill {
        name: "x".into(),
        prompt: "a </skill> b".into(),
        ..Skill::default()
    };
    assert_eq!(sk.block().matches("</skill>").count(), 1);

    // The catalog by name, past its share.
    let cat = vec![
        Entry {
            name: "alpha".into(),
            description: "d".repeat(100),
        },
        Entry {
            name: "beta".into(),
            description: "d".into(),
        },
        Entry {
            name: "gamma".into(),
            description: "d".into(),
        },
    ];
    assert!(catalog_text(&cat, 0).contains("- alpha: ddd"));
    let t = catalog_text(&cat, 60);
    assert!(
        t.contains("by name only") && t.contains("\n- alpha") && !t.contains("ddd"),
        "{t}"
    );
    assert!(catalog_text(&cat, 50).contains("- and 2 more"));
}

/// What each built-in skill does in [`builtin_skills_run`]: a read call, and the change it
/// proposes, when it proposes one, on the stand-in's config.
const RUNS: &[(&str, &str, &str)] = &[
    ("explain", r#"{"members": ["routes"]}"#, ""),
    (
        "route-builder",
        r#"{"members": ["routes", "upstreams", "listeners"]}"#,
        r#"{"summary": "Answer /healthz", "ops": [{"op": "add", "path": "/routes/0", "value": {"name": "health", "match": {"path": ["/healthz"]}, "handler": {"module": "respond", "params": {"body": "ok"}}}}]}"#,
    ),
    (
        "reverse-proxy",
        r#"{"members": ["upstreams"]}"#,
        r#"{"summary": "A second backend", "members": {"upstreams": [{"name": "app", "addresses": ["10.0.0.7:9000", "10.0.0.8:9000"]}]}}"#,
    ),
    ("tls-acme", r#"{"members": ["listeners"]}"#, ""),
    ("rate-limit-auth", r#"{"members": ["routes"]}"#, ""),
    ("cache-tuning", r#"{"members": ["routes"]}"#, ""),
    ("diagnose", r#"{"members": ["routes", "upstreams"]}"#, ""),
    ("drift-explain", "{}", ""),
    ("log-analysis", r#"{"level": "warn"}"#, ""),
    ("security-review", "{}", ""),
    (
        "migrate",
        r#"{"from": "nginx", "text": "http { server { listen 8080; location / { proxy_pass http://10.0.0.1:9000; } } }"}"#,
        r#"{"summary": "The converted site", "ops": [{"op": "add", "path": "/routes/-", "value": {"name": "site", "match": {"path": ["/*"]}, "handler": {"module": "proxy", "params": {"upstream": "app"}}}}]}"#,
    ),
    ("media-setup", r#"{"members": ["listeners", "routes"]}"#, ""),
    ("llm-mesh-setup", r#"{"members": ["listeners"]}"#, ""),
    ("tunnel-setup", r#"{"members": ["listeners"]}"#, ""),
    (
        "vod-setup",
        r#"{"members": ["media_library", "routes"]}"#,
        "",
    ),
    ("channel-scheduler", r#"{"members": ["streaming"]}"#, ""),
    ("sync-pipeline", r#"{"members": ["sync"]}"#, ""),
];

/// fit's `TestBuiltinSkillsRun`: each built-in skill whose first tool akit has, run by `/name`
/// with its first example against a scripted model: its prompt in the system prompt, its tools
/// offered (narrowed to its own and the user's level, without the catalog), its read call
/// answered, and its proposal, when it makes one, valid; the audit log names it. The others wait
/// for the tools [`TOOLS_TO_COME`] names.
#[test]
fn builtin_skills_run() {
    for sk in builtin() {
        if CLUSTER_SKILLS.contains(&sk.name.as_str()) {
            continue;
        }
        let Some((_, read, propose)) = RUNS.iter().find(|(n, _, _)| *n == sk.name) else {
            assert!(
                TOOLS_TO_COME.iter().any(|(t, _)| *t == sk.tools[0]),
                "no run for {}",
                sk.name
            );
            continue;
        };
        let mut turns = vec![calls(&[(sk.tools[0].as_str(), read)])];
        if !propose.is_empty() {
            turns.push(calls(&[("config_propose", propose)]));
        }
        turns.push(text(&format!("Done: {}", sk.title)));
        let f = start(
            r#", "default_level": "propose", "data": {"send_logs": true, "send_access_log": true}"#,
            turns.clone(),
        );
        let body = Obj::new()
            .str("message", &format!("/{} {}", sk.name, sk.examples[0]))
            .end();
        let c = f.chat(&body);
        assert_eq!(
            c.end().map(|e| s(e, "reason")),
            Some("answer"),
            "{}: {:?}",
            sk.name,
            c.err()
        );
        assert_eq!(s(c.of("start")[0], "skill"), sk.name);
        for r in c.of("tool_result") {
            // akit's own log is kept in memory only once `akit run` installs it.
            let no_ring = sk.name == "log-analysis" && s(r, "content").contains("keeps no lines");
            assert!(
                !b(r, "error") || no_ring,
                "{}: {} failed: {}",
                sk.name,
                s(r, "name"),
                s(r, "content")
            );
        }
        let reqs = f.model_calls();
        assert_eq!(reqs.len(), turns.len(), "{}", sk.name);
        let sys = system(&reqs[0]);
        assert!(
            sys.contains(&format!("The current skill is {}", sk.name))
                && sys.contains(&sk.prompt[..60])
                && !sys.contains("Skills you may load")
                && last_user(&reqs[0]) == sk.examples[0],
            "{}: {sys}",
            sk.name
        );
        let settings = f.a.settings();
        let mut want: Vec<String> = vec!["ask_user".to_owned()];
        want.extend(
            f.a.skill_tools(&sk, Level::Propose, &settings)
                .into_iter()
                .filter(|t| t.ok)
                .map(|t| t.name),
        );
        want.sort();
        assert_eq!(offered(&reqs[0]), want, "{}", sk.name);
        let conv = c.conv();
        if !propose.is_empty() {
            let (_, v, body) = f.view(&conv);
            assert!(
                v.is_some_and(|v| b(&v, "valid")),
                "{}: the proposal: {body}",
                sk.name
            );
        }
        let e = f.a.audit_entries(&Filter::default()).0;
        assert!(
            !e.is_empty() && e[0].skill == sk.name && e[0].kind == crate::audit::KIND_CHAT,
            "{}: {e:?}",
            sk.name
        );
        f.no_secrets();
    }
}
