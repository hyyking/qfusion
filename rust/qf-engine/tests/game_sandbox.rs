//! Sandbox and limit behaviour of the gametype script engine.

use std::sync::Arc;

use qf_engine::game::{GameHost, Team, Value};
use qf_engine::simulation::pmove::{PmTrace, PmoveWorld, Vec3};

struct Empty;

impl PmoveWorld for Empty {
    fn trace(&self, _: Vec3, _: Vec3, _: Vec3, end: Vec3, _: u32) -> PmTrace {
        PmTrace::miss(end)
    }

    fn point_contents(&self, _: Vec3) -> u32 {
        0
    }
}

fn host(script: &str) -> GameHost {
    GameHost::new(script, Arc::new(Empty), &[], &[], None, 1).unwrap()
}

#[test]
fn runaway_loop_aborts_deterministically_and_reports_a_fault() {
    let mut host = host("fn init(g) {} fn tick(g) { loop {} }");
    let first = host.tick().unwrap_err().to_string();
    let second = host.tick().unwrap_err().to_string();
    assert_eq!(first, second);
    assert_eq!(host.faults(), 2);
    let faults: Vec<_> = host
        .take_events()
        .into_iter()
        .filter(|e| e.kind == "script_fault")
        .collect();
    assert_eq!(faults.len(), 2);
    assert_eq!(
        faults[0].fields.get("hook"),
        Some(&Value::Str("tick".into()))
    );
    assert_eq!(faults[1].int("tick"), Some(2));
}

#[test]
fn host_survives_a_faulting_hook() {
    let mut host = host(
        "fn init(g) {} fn tick(g) { if this.n == () { this.n = 0; } this.n += 1; if this.n == 1 { loop {} } }",
    );
    assert!(host.tick().is_err());
    host.tick().unwrap();
}

#[test]
fn clock_eval_and_modules_are_unavailable() {
    for body in ["let t = timestamp();", "eval(\"1\");", "import \"x\" as m;"] {
        let script = format!("fn init(g) {{}} fn tick(g) {{ {body} }}");
        let outcome = GameHost::new(&script, Arc::new(Empty), &[], &[], None, 1)
            .and_then(|mut host| host.tick());
        assert!(outcome.is_err(), "`{body}` must not run");
    }
}

#[test]
fn oversized_data_is_rejected() {
    let mut host =
        host("fn init(g) {} fn tick(g) { let a = []; for i in 0..100000 { a.push(i); } }");
    assert!(host.tick().is_err());
}

#[test]
fn deep_recursion_is_rejected() {
    let mut host = host("fn init(g) {} fn r(n) { r(n + 1) } fn tick(g) { r(0); }");
    assert!(host.tick().is_err());
    host.add_player(Team::Alpha).unwrap();
}

#[test]
fn hot_reload_swaps_rules_keeps_state_and_rejects_bad_scripts() {
    let mut host = host(
        "fn init(g) { this.n = 0; } fn tick(g) { this.n += 1; g.emit(\"old\", #{ n: this.n }); }",
    );
    host.tick().unwrap();
    let before = host.state_hash();

    let missing = host.reload_script("fn init(g) {}").unwrap_err().to_string();
    assert!(missing.contains("tick"), "{missing}");
    assert!(host.reload_script("fn tick(g) {").is_err());
    assert_eq!(host.reloads(), 0);
    assert_eq!(host.state_hash(), before, "a failed reload changes nothing");
    host.tick().unwrap();

    host.reload_script(
        "fn init(g) {} fn on_reload(g, old) { g.emit(\"reloaded\", #{ old: old, n: this.n }); } \
         fn tick(g) { this.n += 10; g.emit(\"new\", #{ n: this.n }); }",
    )
    .unwrap();
    host.tick().unwrap();
    let events = host.take_events();
    let kinds: Vec<_> = events.iter().map(|e| e.kind.as_str()).collect();
    assert_eq!(kinds, ["old", "old", "reloaded", "new"]);
    assert_eq!(events[2].int("old"), Some(0));
    assert_eq!(events[2].int("n"), Some(2), "script state survives");
    assert_eq!(events[3].int("n"), Some(12));
    assert_eq!(host.reloads(), 1);
}
