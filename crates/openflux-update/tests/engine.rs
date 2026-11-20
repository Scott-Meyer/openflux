mod common;

use common::*;
use openflux_update::engine::{
    ComponentState, Engine, GapEffect, Inventory, PlanRequest, Preferences, Problem, Refusal,
    StepReason, Unmet,
};
use openflux_update::manifest::Release;
use openflux_update::version::Component::{self, Bridge, Display, Firmware, Host};

const STABLE: Preferences = Preferences { beta: false };
const BETA: Preferences = Preferences { beta: true };

#[derive(Debug)]
enum Want {
    UpToDate,
    Update(&'static str, bool),
    NeedsHost(&'static str, &'static str),
    Newer(&'static [(&'static str, GapEffect)]),
    Blocked(&'static str),
    Incompatible,
}

fn matches(state: &ComponentState, want: &Want) -> bool {
    match (state, want) {
        (ComponentState::UpToDate, Want::UpToDate) => true,
        (ComponentState::UpdateAvailable { to, required }, Want::Update(v, r)) => {
            *to == ver(v) && required == r
        }
        (ComponentState::NeedsNewerHost { to, host }, Want::NeedsHost(v, h)) => {
            *to == ver(v) && *host == ver(h)
        }
        (ComponentState::NewerThanHost { gaps }, Want::Newer(expected)) => {
            gaps.len() == expected.len()
                && expected
                    .iter()
                    .all(|(i, e)| gaps.iter().any(|g| g.interface == *i && g.effect == *e))
        }
        (ComponentState::UpdateBlocked { to, .. }, Want::Blocked(v)) => *to == ver(v),
        (ComponentState::Incompatible { .. }, Want::Incompatible) => true,
        _ => false,
    }
}

struct Case {
    name: &'static str,
    profile: openflux_update::HostProfile,
    inventory: Inventory,
    releases: Vec<Release>,
    prefs: Preferences,
    want: [(Component, Want); 4],
}

fn host_release(v: &str, interfaces: openflux_update::InterfaceSet) -> Release {
    release(Host, v, interfaces)
}

fn status_cases() -> Vec<Case> {
    let fw_keymap = |k: &str| {
        set(
            &[
                ("firmware-usb", &["1.0"]),
                ("uart", &["1.0"]),
                ("keymap", &[k]),
            ],
            &[],
        )
    };
    let br_usb = |u: &'static [&'static str]| {
        set(
            &[("bridge-usb", u), ("display-ipc", &["1.0"])],
            &[("uart", &["1.0"])],
        )
    };
    vec![
        Case {
            profile: profile(),
            name: "everything current, no releases",
            inventory: baseline(),
            releases: vec![],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "compatible bridge release is an available update",
            inventory: baseline(),
            releases: vec![
                release(Bridge, "1.1.0", br_set()),
                release(Bridge, "1.0.0", br_set()),
            ],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::Update("1.1.0", false)),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "beta is not offered without opting in",
            inventory: baseline(),
            releases: vec![
                release(Bridge, "1.1.0-beta.1", br_set()),
                release(Host, "1.1.0-beta.1", host_set()),
            ],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "beta is offered after opting in",
            inventory: baseline(),
            releases: vec![
                release(Bridge, "1.1.0-beta.1", br_set()),
                release(Host, "1.1.0-beta.1", host_set()),
            ],
            prefs: BETA,
            want: [
                (Host, Want::Update("1.1.0-beta.1", false)),
                (Firmware, Want::UpToDate),
                (Bridge, Want::Update("1.1.0-beta.1", false)),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "prereleases order numerically and the final release beats them",
            inventory: Inventory {
                bridge: openflux("1.1.0-beta.2", br_set()),
                ..baseline()
            },
            releases: vec![
                release(Bridge, "1.1.0-beta.1", br_set()),
                release(Bridge, "1.1.0-beta.10", br_set()),
                release(Bridge, "1.1.0", br_set()),
            ],
            prefs: BETA,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::Update("1.1.0", false)),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "beta.10 is newer than beta.2",
            inventory: Inventory {
                bridge: openflux("1.1.0-beta.2", br_set()),
                ..baseline()
            },
            releases: vec![
                release(Bridge, "1.1.0-beta.1", br_set()),
                release(Bridge, "1.1.0-beta.10", br_set()),
            ],
            prefs: BETA,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::Update("1.1.0-beta.10", false)),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "a beta install leaving the beta channel waits for the next stable release",
            inventory: Inventory {
                bridge: openflux("1.1.0-beta.2", br_set()),
                ..baseline()
            },
            releases: vec![
                release(Bridge, "1.1.0-beta.10", br_set()),
                release(Bridge, "1.0.5", br_set()),
            ],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "firmware with a newer keymap minor needs a host that understands it",
            inventory: baseline(),
            releases: vec![
                release(Firmware, "1.1.0", fw_keymap("1.1")),
                host_release(
                    "1.1.0",
                    with_built(host_set(), &[("keymap", "1.1"), ("config", "1.0")]),
                ),
            ],
            prefs: STABLE,
            want: [
                (Host, Want::Update("1.1.0", false)),
                (Firmware, Want::NeedsHost("1.1.0", "1.1.0")),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "keyboard data newer than the host is read-only",
            inventory: Inventory {
                firmware: openflux("1.3.0", fw_keymap("1.3")),
                ..baseline()
            },
            releases: vec![],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::Newer(&[("keymap", GapEffect::ReadOnly)])),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "a newer protocol minor is not a gap",
            inventory: Inventory {
                bridge: openflux("1.4.0", br_usb(&["1.4"])),
                ..baseline()
            },
            releases: vec![],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "keyboard speaking a newer major than the host is newer than host",
            inventory: Inventory {
                bridge: openflux("2.0.0", br_usb(&["2.0"])),
                ..baseline()
            },
            // A display release exists, but the host cannot reach the display through this bridge.
            releases: vec![release(Display, "1.1.0", dp_set())],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::UpToDate),
                (
                    Bridge,
                    Want::Newer(&[("bridge-usb", GapEffect::Unavailable)]),
                ),
                (Display, Want::Blocked("1.1.0")),
            ],
        },
        Case {
            name: "firmware too old for the host with no fix is incompatible",
            profile: {
                let mut p = profile();
                p.interfaces
                    .requires
                    .insert("keymap".into(), vec![req("1.2")]);
                p
            },
            inventory: Inventory {
                firmware: openflux("1.0.0", fw_keymap("1.0")),
                ..baseline()
            },
            releases: vec![],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::Incompatible),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
        Case {
            profile: profile(),
            name: "firmware that drops the uart major the bridge needs is blocked",
            inventory: baseline(),
            releases: vec![release(
                Firmware,
                "2.0.0",
                set(
                    &[
                        ("firmware-usb", &["1.0"]),
                        ("uart", &["2.0"]),
                        ("keymap", &["1.0"]),
                    ],
                    &[],
                ),
            )],
            prefs: STABLE,
            want: [
                (Host, Want::UpToDate),
                (Firmware, Want::Blocked("2.0.0")),
                (Bridge, Want::UpToDate),
                (Display, Want::UpToDate),
            ],
        },
    ]
}

#[test]
fn status_table() {
    let mut failures = Vec::new();
    for case in status_cases() {
        let cat = catalog(case.releases.clone());
        let engine = Engine::new(&case.profile, &case.inventory, &cat, case.prefs);
        let statuses = engine.status();
        for (c, want) in &case.want {
            let got = statuses.iter().find(|s| s.component == *c).unwrap();
            if !matches(&got.state, want) {
                failures.push(format!(
                    "{}: {c}: want {want:?}, got {:?}",
                    case.name, got.state
                ));
            }
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn incompatible_firmware_with_a_fix_is_a_required_update() {
    let mut p = profile();
    p.interfaces
        .requires
        .insert("keymap".into(), vec![req("1.2")]);
    let inv = baseline();
    let fixed = set(
        &[
            ("firmware-usb", &["1.0"]),
            ("uart", &["1.0"]),
            ("keymap", &["1.2"]),
        ],
        &[],
    );
    let mut p_built = p.clone();
    p_built
        .interfaces
        .built_with
        .insert("keymap".into(), iv("1.2"));
    let cat = catalog(vec![release(Firmware, "1.2.0", fixed)]);
    let s = Engine::new(&p_built, &inv, &cat, STABLE).component_status(Firmware);
    assert_eq!(
        s.state,
        ComponentState::UpdateAvailable {
            to: ver("1.2.0"),
            required: true
        }
    );
    assert_eq!(s.unmet.len(), 1);
    assert_eq!(s.unmet[0].interface, "keymap");
}

#[test]
fn withdrawn_releases_are_never_offered_and_are_flagged_when_installed() {
    let p = profile();
    let mut withdrawn = release(Bridge, "1.1.0", br_set());
    withdrawn.withdrawn = true;
    let cat = catalog(vec![withdrawn.clone()]);
    let inv = baseline();
    let e = Engine::new(&p, &inv, &cat, STABLE);
    assert_eq!(e.component_status(Bridge).state, ComponentState::UpToDate);
    assert_eq!(
        e.plan(&PlanRequest::only(Bridge)),
        Err(Refusal::NothingToUpdate)
    );
    assert_eq!(
        e.plan_downgrade(Bridge, &ver("1.1.0")).unwrap_err(),
        Refusal::NoSuchRelease {
            component: Bridge,
            version: ver("1.1.0")
        }
    );

    let inv = Inventory {
        bridge: openflux("1.1.0", br_set()),
        ..baseline()
    };
    let s = Engine::new(&p, &inv, &cat, STABLE).component_status(Bridge);
    assert!(s.installed_withdrawn);
}

#[test]
fn update_available_reports_a_further_update_after_the_host() {
    let p = profile();
    let inv = baseline();
    let cat = catalog(vec![
        release(Firmware, "1.0.5", fw_set()),
        release(
            Firmware,
            "1.1.0",
            set(
                &[
                    ("firmware-usb", &["1.0"]),
                    ("uart", &["1.0"]),
                    ("keymap", &["1.1"]),
                ],
                &[],
            ),
        ),
        release(
            Host,
            "1.1.0",
            with_built(host_set(), &[("keymap", "1.1"), ("config", "1.0")]),
        ),
    ]);
    let s = Engine::new(&p, &inv, &cat, STABLE).component_status(Firmware);
    assert_eq!(
        s.state,
        ComponentState::UpdateAvailable {
            to: ver("1.0.5"),
            required: false
        }
    );
    let gate = s.after_host_update.expect("host gate");
    assert_eq!((gate.to, gate.host), (ver("1.1.0"), ver("1.1.0")));
}

fn steps(plan: &openflux_update::Plan) -> Vec<(Component, String, StepReason)> {
    plan.steps
        .iter()
        .map(|s| (s.component, s.to.to_string(), s.reason))
        .collect()
}

#[test]
fn bridge_update_that_would_strand_the_display_is_refused() {
    let p = profile();
    let inv = baseline();
    let br2 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["2.0"])],
        &[("uart", &["1.0"])],
    );
    let cat = catalog(vec![release(Bridge, "2.0.0", br2)]);
    let err = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Bridge))
        .unwrap_err();
    let Refusal::NoSafePath { blockers } = err else {
        panic!("{err:?}")
    };
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].version, ver("2.0.0"));
    assert!(matches!(
        &blockers[0].problems[..],
        [Problem::Breaks { unmet }] if unmet.consumer == Display && unmet.interface == "display-ipc"
    ));
}

#[test]
fn bridge_update_brings_a_matching_display_along() {
    let p = profile();
    let inv = baseline();
    let br2 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["2.0"])],
        &[("uart", &["1.0"])],
    );
    let dp2 = set(&[("config", &["1.0"])], &[("display-ipc", &["2.0"])]);
    let cat = catalog(vec![
        release(Bridge, "2.0.0", br2),
        release(Display, "2.0.0", dp2),
    ]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Bridge))
        .unwrap();
    let s = steps(&plan);
    assert_eq!(s.len(), 2);
    assert!(s.contains(&(Bridge, "2.0.0".into(), StepReason::Requested)));
    assert!(s.contains(&(Display, "2.0.0".into(), StepReason::Companion)));
    // No order avoids a moment where display and bridge disagree; the plan says so.
    assert_eq!(plan.steps[0].temporarily_unmet.len(), 1);
    assert_eq!(plan.steps[0].temporarily_unmet[0].interface, "display-ipc");
    assert!(plan.steps[1].temporarily_unmet.is_empty());
}

#[test]
fn transitional_bridge_avoids_breaking_the_display() {
    let p = profile();
    let inv = baseline();
    let br19 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.5", "2.0"])],
        &[("uart", &["1.0"])],
    );
    let br2 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["2.0"])],
        &[("uart", &["1.0"])],
    );
    let dp2 = set(&[("config", &["1.0"])], &[("display-ipc", &["2.0"])]);
    let cat = catalog(vec![
        release(Bridge, "1.9.0", br19),
        release(Bridge, "2.0.0", br2),
        release(Display, "2.0.0", dp2),
    ]);
    let engine = Engine::new(&p, &inv, &cat, STABLE);
    let plan = engine.plan(&PlanRequest::keyboard()).unwrap();
    assert_eq!(
        steps(&plan),
        vec![
            (Bridge, "1.9.0".into(), StepReason::Transitional),
            (Display, "2.0.0".into(), StepReason::Requested),
            (Bridge, "2.0.0".into(), StepReason::Requested),
        ]
    );
    assert!(plan.steps.iter().all(|s| s.temporarily_unmet.is_empty()));

    // Executing the first step and planning again yields the rest of the plan.
    let after = Inventory {
        bridge: openflux("1.9.0", plan.steps[0].release.interfaces.clone()),
        ..baseline()
    };
    let rest = Engine::new(&p, &after, &cat, STABLE)
        .plan(&PlanRequest::keyboard())
        .unwrap();
    assert_eq!(steps(&rest), steps(&plan)[1..].to_vec());

    // Deterministic.
    assert_eq!(engine.plan(&PlanRequest::keyboard()).unwrap(), plan);
}

#[test]
fn firmware_dropping_a_uart_major_updates_the_bridge_first() {
    let p = profile();
    let inv = baseline();
    let fw2 = set(
        &[
            ("firmware-usb", &["1.0"]),
            ("uart", &["2.0"]),
            ("keymap", &["1.0"]),
        ],
        &[],
    );
    let br15 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.0"])],
        &[("uart", &["1.0", "2.0"])],
    );
    let cat = catalog(vec![
        release(Firmware, "2.0.0", fw2),
        release(Bridge, "1.5.0", br15),
    ]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Firmware))
        .unwrap();
    assert_eq!(
        steps(&plan),
        vec![
            (Bridge, "1.5.0".into(), StepReason::Companion),
            (Firmware, "2.0.0".into(), StepReason::Requested)
        ]
    );
    assert!(plan.steps.iter().all(|s| s.temporarily_unmet.is_empty()));
}

#[test]
fn order_is_searched_not_fixed() {
    // Here the firmware must go first: the new bridge only speaks uart 2.
    let p = profile();
    let inv = baseline();
    let fw2 = set(
        &[
            ("firmware-usb", &["1.0"]),
            ("uart", &["1.0", "2.0"]),
            ("keymap", &["1.0"]),
        ],
        &[],
    );
    let br2 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.0"])],
        &[("uart", &["2.0"])],
    );
    let cat = catalog(vec![
        release(Firmware, "2.0.0", fw2),
        release(Bridge, "2.0.0", br2),
    ]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::keyboard())
        .unwrap();
    assert_eq!(
        steps(&plan),
        vec![
            (Firmware, "2.0.0".into(), StepReason::Requested),
            (Bridge, "2.0.0".into(), StepReason::Requested)
        ]
    );
}

#[test]
fn host_moving_to_a_new_transport_major_goes_through_a_transitional_bridge() {
    let p = profile();
    let inv = baseline();
    let host2 = {
        let mut s = host_set();
        s.requires.insert("bridge-usb".into(), vec![req("2.0")]);
        s.built_with.insert("bridge-usb".into(), iv("2.0"));
        s
    };
    let br19 = set(
        &[("bridge-usb", &["1.9", "2.0"]), ("display-ipc", &["1.0"])],
        &[("uart", &["1.0"])],
    );
    let br2 = set(
        &[("bridge-usb", &["2.0"]), ("display-ipc", &["1.0"])],
        &[("uart", &["1.0"])],
    );
    let all = vec![
        release(Host, "2.0.0", host2.clone()),
        release(Bridge, "1.9.0", br19),
        release(Bridge, "2.0.0", br2.clone()),
    ];

    let plan = Engine::new(&p, &inv, &catalog(all.clone()), STABLE)
        .plan(&PlanRequest::everything())
        .unwrap();
    assert_eq!(
        steps(&plan),
        vec![
            (Bridge, "1.9.0".into(), StepReason::Transitional),
            (Host, "2.0.0".into(), StepReason::Requested),
            (Bridge, "2.0.0".into(), StepReason::Requested),
        ]
    );

    // The bridge status says a newer host comes first.
    let s = Engine::new(&p, &inv, &catalog(all), STABLE).component_status(Bridge);
    assert_eq!(
        s.state,
        ComponentState::UpdateAvailable {
            to: ver("1.9.0"),
            required: false
        }
    );
    assert_eq!(
        s.after_host_update.map(|g| (g.to, g.host)),
        Some((ver("2.0.0"), ver("2.0.0")))
    );

    // Without the transitional bridge either order strands the bridge.
    let cat = catalog(vec![
        release(Host, "2.0.0", host2),
        release(Bridge, "2.0.0", br2),
    ]);
    let err = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::everything())
        .unwrap_err();
    let Refusal::NoSafePath { blockers } = err else {
        panic!("{err:?}")
    };
    for b in &blockers {
        assert!(
            b.problems.iter().any(|p| matches!(
                p,
                Problem::LosesTransport {
                    component: Bridge,
                    ..
                }
            )),
            "{b:?}"
        );
    }
    assert!(blockers.iter().any(|b| b.component == Host));
    assert!(blockers.iter().any(|b| b.component == Bridge));
}

#[test]
fn keyboard_newer_than_host_cannot_be_touched() {
    let p = profile();
    let inv = Inventory {
        bridge: openflux(
            "2.0.0",
            set(
                &[("bridge-usb", &["2.0"]), ("display-ipc", &["1.0"])],
                &[("uart", &["1.0"])],
            ),
        ),
        ..baseline()
    };
    let cat = catalog(vec![
        release(Bridge, "2.1.0", br_set()),
        release(Display, "1.1.0", dp_set()),
    ]);
    let e = Engine::new(&p, &inv, &cat, STABLE);
    assert!(matches!(
        e.plan(&PlanRequest::only(Bridge)),
        Err(Refusal::NoTransport {
            component: Bridge,
            ..
        })
    ));
    assert!(matches!(
        e.plan(&PlanRequest::only(Display)),
        Err(Refusal::NoTransport {
            component: Display,
            ..
        })
    ));
    // Firmware is reachable over its own transport.
    let cat = catalog(vec![release(Firmware, "1.1.0", fw_set())]);
    let e = Engine::new(&p, &inv, &cat, STABLE);
    assert_eq!(
        steps(&e.plan(&PlanRequest::only(Firmware)).unwrap()),
        vec![(Firmware, "1.1.0".into(), StepReason::Requested)]
    );
}

#[test]
fn existing_problems_do_not_block_unrelated_updates() {
    let mut p = profile();
    p.interfaces
        .requires
        .insert("keymap".into(), vec![req("1.2")]);
    let inv = baseline();
    let cat = catalog(vec![release(Display, "1.1.0", dp_set())]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Display))
        .unwrap();
    assert_eq!(plan.steps.len(), 1);
    assert_eq!(plan.remaining_unmet.len(), 1);
    assert_eq!(plan.remaining_unmet[0].interface, "keymap");
}

#[test]
fn downgrades_are_explicit_and_validated() {
    let p = profile();
    let br11 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.1"])],
        &[("uart", &["1.0"])],
    );
    let dp11 = set(&[("config", &["1.0"])], &[("display-ipc", &["1.1"])]);
    let cat = catalog(vec![
        release(Bridge, "1.0.0", br_set()),
        release(Bridge, "1.1.0", br11.clone()),
        release(Bridge, "1.0.1", br11.clone()),
        release(Bridge, "1.2.0", br11.clone()),
    ]);

    // Never proposed as an update.
    let inv = Inventory {
        bridge: openflux("1.1.0", br11.clone()),
        ..baseline()
    };
    let e = Engine::new(&p, &inv, &cat, STABLE);
    let plan = e.plan(&PlanRequest::only(Bridge)).unwrap();
    assert_eq!(
        steps(&plan),
        vec![(Bridge, "1.2.0".into(), StepReason::Requested)]
    );

    let down = e.plan_downgrade(Bridge, &ver("1.0.1")).unwrap();
    assert_eq!(
        steps(&down),
        vec![(Bridge, "1.0.1".into(), StepReason::Downgrade)]
    );
    assert_eq!(down.steps[0].from, ver("1.1.0"));
    assert_eq!(
        e.plan_downgrade(Bridge, &ver("1.2.0")).unwrap_err(),
        Refusal::NotADowngrade {
            component: Bridge,
            version: ver("1.2.0")
        }
    );
    assert_eq!(
        e.plan_downgrade(Bridge, &ver("0.9.0")).unwrap_err(),
        Refusal::NoSuchRelease {
            component: Bridge,
            version: ver("0.9.0")
        }
    );
    assert_eq!(
        e.plan_downgrade(Host, &ver("0.9.0")).unwrap_err(),
        Refusal::NotAKeyboardComponent { component: Host }
    );

    // A downgrade that would break the display is refused.
    let inv = Inventory {
        bridge: openflux("1.1.0", br11),
        display: openflux("1.1.0", dp11),
        ..baseline()
    };
    let e = Engine::new(&p, &inv, &cat, STABLE);
    let err = e.plan_downgrade(Bridge, &ver("1.0.0")).unwrap_err();
    let Refusal::UnsafeDowngrade { problems, .. } = err else {
        panic!("{err:?}")
    };
    assert!(matches!(&problems[..], [Problem::Breaks { unmet }] if unmet.consumer == Display));
}

#[test]
fn nothing_to_update_when_current() {
    let p = profile();
    let inv = baseline();
    let cat = catalog(vec![release(Bridge, "1.0.0", br_set())]);
    assert_eq!(
        Engine::new(&p, &inv, &cat, STABLE).plan(&PlanRequest::everything()),
        Err(Refusal::NothingToUpdate)
    );
}

#[test]
fn companions_are_not_updated_without_need() {
    let p = profile();
    let inv = baseline();
    let cat = catalog(vec![
        release(Bridge, "1.1.0", br_set()),
        release(Display, "1.1.0", dp_set()),
    ]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Bridge))
        .unwrap();
    assert_eq!(
        steps(&plan),
        vec![(Bridge, "1.1.0".into(), StepReason::Requested)]
    );
}

#[test]
fn held_back_explains_what_the_plan_does_not_reach() {
    let p = profile();
    let inv = baseline();
    let br2 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["2.0"])],
        &[("uart", &["1.0"])],
    );
    let cat = catalog(vec![
        release(Bridge, "1.1.0", br_set()),
        release(Bridge, "2.0.0", br2),
    ]);
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Bridge))
        .unwrap();
    assert_eq!(
        steps(&plan),
        vec![(Bridge, "1.1.0".into(), StepReason::Requested)]
    );
    assert_eq!(plan.held_back.len(), 1);
    assert_eq!(plan.held_back[0].version, ver("2.0.0"));
}

#[test]
fn status_and_plans_serialize_for_the_ui() {
    let p = profile();
    let inv = baseline();
    let cat = catalog(vec![release(Bridge, "1.1.0", br_set())]);
    let e = Engine::new(&p, &inv, &cat, STABLE);
    let json = serde_json::to_value(e.status()).unwrap();
    assert_eq!(json[2]["component"], "bridge");
    assert_eq!(json[2]["state"]["state"], "update-available");
    assert_eq!(json[2]["state"]["to"], "1.1.0");
    let plan = serde_json::to_value(e.plan(&PlanRequest::only(Bridge)).unwrap()).unwrap();
    assert_eq!(plan["steps"][0]["reason"], "requested");
    let refusal = serde_json::to_value(e.plan(&PlanRequest::only(Firmware)).unwrap_err()).unwrap();
    assert_eq!(refusal["refusal"], "nothing-to-update");
}

#[test]
fn unmet_requirements_name_the_provider_or_its_absence() {
    let unmet = |provider, provided: &[&str]| Unmet {
        consumer: Display,
        interface: "display-ipc".into(),
        requires: vec![req("1.2")],
        provider,
        provided: provided.iter().map(|v| iv(v)).collect(),
    };
    assert_eq!(
        unmet(None, &[]).to_string(),
        "display needs display-ipc >=1.2, <2.0 but no component provides it"
    );
    assert_eq!(
        unmet(Some(Bridge), &[]).to_string(),
        "display needs display-ipc >=1.2, <2.0 but bridge does not provide it"
    );
    assert_eq!(
        unmet(Some(Bridge), &["1.1"]).to_string(),
        "display needs display-ipc >=1.2, <2.0 but bridge provides 1.1"
    );
}
