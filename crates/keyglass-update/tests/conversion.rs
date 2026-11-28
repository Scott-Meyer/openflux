//! Keyboards that are not fully Keyglass: the only keyboard action is
//! converting the whole keyboard, and host updates stay available.

mod common;

use std::collections::BTreeSet;

use common::*;
use keyglass_update::engine::{
    ComponentState, Engine, InstalledSoftware, Inventory, KeyboardMode, PlanRequest, Preferences,
    Refusal, SoftwareKind, StepReason,
};
use keyglass_update::version::Component::{self, Bridge, Display, Firmware, Host};

const STABLE: Preferences = Preferences { beta: false };

fn other(kind: SoftwareKind) -> InstalledSoftware {
    match kind {
        SoftwareKind::DevelopmentBuild => InstalledSoftware::DevelopmentBuild {
            label: "development build".into(),
        },
        SoftwareKind::Factory => InstalledSoftware::Factory {
            label: "factory".into(),
        },
        SoftwareKind::Unknown => InstalledSoftware::Unknown {
            label: "no answer".into(),
        },
        SoftwareKind::Keyglass => unreachable!(),
    }
}

fn with(c: Component, software: InstalledSoftware) -> Inventory {
    let mut inv = baseline();
    match c {
        Firmware => inv.firmware = software,
        Bridge => inv.bridge = software,
        Display => inv.display = software,
        Host => unreachable!(),
    }
    inv
}

/// Every keyboard with exactly one non-Keyglass component, plus fully
/// non-Keyglass keyboards.
fn mixed_keyboards() -> Vec<(String, Inventory)> {
    let kinds = [
        SoftwareKind::Factory,
        SoftwareKind::DevelopmentBuild,
        SoftwareKind::Unknown,
    ];
    let mut out = Vec::new();
    for kind in kinds {
        for c in Component::DEVICE {
            out.push((format!("{c} runs {kind:?}"), with(c, other(kind))));
        }
        out.push((
            format!("everything runs {kind:?}"),
            Inventory {
                firmware: other(kind),
                bridge: other(kind),
                display: other(kind),
            },
        ));
    }
    out
}

/// Releases that would be ordinary updates on an Keyglass keyboard.
fn releases() -> Vec<keyglass_update::Release> {
    vec![
        release(Host, "1.1.0", host_set()),
        release(Firmware, "1.1.0", fw_set()),
        release(Bridge, "1.1.0", br_set()),
        release(Display, "1.1.0", dp_set()),
    ]
}

fn all_requests() -> Vec<PlanRequest> {
    let mut out = Vec::new();
    for mask in 1u8..16 {
        let components: BTreeSet<Component> = Component::ALL
            .into_iter()
            .enumerate()
            .filter(|(i, _)| mask & (1 << i) != 0)
            .map(|(_, c)| c)
            .collect();
        out.push(PlanRequest { components });
    }
    out
}

#[test]
fn any_non_keyglass_component_means_conversion_only() {
    let p = profile();
    let cat = catalog(releases());
    for (name, inv) in mixed_keyboards() {
        let e = Engine::new(&p, &inv, &cat, STABLE);
        let KeyboardMode::NeedsConversion { found } = e.mode() else {
            panic!("{name}: not in conversion mode")
        };
        assert_eq!(found.len(), 3, "{name}");

        for s in e.status() {
            match s.component {
                Host => assert_eq!(
                    s.state,
                    ComponentState::UpdateAvailable {
                        to: ver("1.1.0"),
                        required: false
                    },
                    "{name}"
                ),
                c => assert_eq!(
                    s.state,
                    ComponentState::ConversionRequired { found: found[&c] },
                    "{name}: {c}"
                ),
            }
        }

        for request in all_requests() {
            let got = e.plan(&request);
            if request.components == BTreeSet::from([Host]) {
                let plan = got.unwrap_or_else(|r| panic!("{name}: host update refused: {r}"));
                assert_eq!(plan.steps.len(), 1, "{name}");
                assert_eq!(plan.steps[0].component, Host);
                assert_eq!(plan.steps[0].reason, StepReason::Requested);
            } else {
                assert_eq!(
                    got,
                    Err(Refusal::ConversionRequired {
                        found: found.clone()
                    }),
                    "{name}: {request:?}"
                );
            }
        }
        for c in Component::DEVICE {
            assert!(
                matches!(
                    e.plan_downgrade(c, &ver("1.0.0")),
                    Err(Refusal::ConversionRequired { .. })
                ),
                "{name}: {c}"
            );
        }

        let conversion = e.propose_conversion().unwrap();
        let installs: Vec<(Component, String)> = conversion
            .install
            .iter()
            .map(|r| (r.component, r.version.to_string()))
            .collect();
        assert_eq!(
            installs,
            vec![
                (Firmware, "1.1.0".into()),
                (Bridge, "1.1.0".into()),
                (Display, "1.1.0".into())
            ],
            "{name}"
        );
        assert_eq!(conversion.found.len(), 3);
    }
}

#[test]
fn refusal_names_what_needs_converting() {
    let p = profile();
    let cat = catalog(releases());
    let inv = with(Bridge, other(SoftwareKind::Factory));
    let err = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Firmware))
        .unwrap_err();
    assert_eq!(
        err.to_string(),
        "bridge runs factory software; convert the keyboard to Keyglass first"
    );
    let json = serde_json::to_value(&err).unwrap();
    assert_eq!(json["refusal"], "conversion-required");
    assert_eq!(json["found"]["bridge"], "factory");
    assert_eq!(json["found"]["firmware"], "keyglass");
}

#[test]
fn host_updates_need_no_keyboard_reasoning_during_conversion() {
    // A host that would not talk to the installed bridge is offered anyway: the
    // keyboard is about to be converted to releases that match it.
    let p = profile();
    let mut host2 = host_set();
    host2.requires.insert("bridge-usb".into(), vec![req("2.0")]);
    let cat = catalog(vec![release(Host, "2.0.0", host2)]);
    let inv = with(Display, other(SoftwareKind::Unknown));
    let plan = Engine::new(&p, &inv, &cat, STABLE)
        .plan(&PlanRequest::only(Host))
        .unwrap();
    assert_eq!(plan.steps[0].to, ver("2.0.0"));

    // Nothing newer: nothing to update.
    let cat = catalog(vec![release(Host, "1.0.0", host_set())]);
    assert_eq!(
        Engine::new(&p, &inv, &cat, STABLE).plan(&PlanRequest::only(Host)),
        Err(Refusal::NothingToUpdate)
    );
}

#[test]
fn the_normal_updater_never_converts() {
    let p = profile();
    let cat = catalog(releases());

    // On an Keyglass keyboard there is nothing to convert.
    let inv = baseline();
    let e = Engine::new(&p, &inv, &cat, STABLE);
    assert_eq!(e.mode(), KeyboardMode::Keyglass);
    assert_eq!(e.propose_conversion(), Err(Refusal::AlreadyKeyglass));

    // On a keyboard that needs conversion, no request to the updater ever
    // yields a keyboard step: conversion is only available through
    // propose_conversion.
    for (name, inv) in mixed_keyboards() {
        let e = Engine::new(&p, &inv, &cat, STABLE);
        for request in all_requests() {
            if let Ok(plan) = e.plan(&request) {
                assert!(
                    plan.steps.iter().all(|s| s.component == Host),
                    "{name}: {request:?}"
                );
            }
        }
    }
}

#[test]
fn conversion_proposal_picks_releases_that_work_together() {
    let p = profile();
    let inv = Inventory {
        firmware: other(SoftwareKind::Factory),
        bridge: other(SoftwareKind::Factory),
        display: other(SoftwareKind::Factory),
    };
    let br11 = set(
        &[("bridge-usb", &["1.0"]), ("display-ipc", &["1.1"])],
        &[("uart", &["1.0"])],
    );
    let dp11 = set(&[("config", &["1.0"])], &[("display-ipc", &["1.1"])]);
    let dp12 = set(&[("config", &["1.0"])], &[("display-ipc", &["1.2"])]);
    let mut withdrawn = release(Firmware, "1.3.0", fw_set());
    withdrawn.withdrawn = true;
    type Case = (
        &'static str,
        Vec<keyglass_update::Release>,
        Preferences,
        Result<[&'static str; 3], &'static str>,
    );
    let cases: Vec<Case> = vec![
        (
            "newest that work together",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                release(Bridge, "1.0.0", br_set()),
                release(Bridge, "1.1.0", br11.clone()),
                release(Display, "1.1.0", dp11.clone()),
                release(Display, "1.2.0", dp12.clone()),
            ],
            STABLE,
            Ok(["1.0.0", "1.1.0", "1.1.0"]),
        ),
        (
            "beta only when opted in",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                release(Firmware, "1.1.0-beta.1", fw_set()),
                release(Bridge, "1.0.0", br_set()),
                release(Display, "1.0.0", dp_set()),
            ],
            STABLE,
            Ok(["1.0.0", "1.0.0", "1.0.0"]),
        ),
        (
            "beta after opting in",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                release(Firmware, "1.1.0-beta.1", fw_set()),
                release(Bridge, "1.0.0", br_set()),
                release(Display, "1.0.0", dp_set()),
            ],
            Preferences { beta: true },
            Ok(["1.1.0-beta.1", "1.0.0", "1.0.0"]),
        ),
        (
            "withdrawn releases are skipped",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                withdrawn,
                release(Bridge, "1.0.0", br_set()),
                release(Display, "1.0.0", dp_set()),
            ],
            STABLE,
            Ok(["1.0.0", "1.0.0", "1.0.0"]),
        ),
        (
            "no display release",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                release(Bridge, "1.0.0", br_set()),
            ],
            STABLE,
            Err("NoRelease"),
        ),
        (
            "nothing fits together",
            vec![
                release(Firmware, "1.0.0", fw_set()),
                release(Bridge, "1.0.0", br_set()),
                release(Display, "1.2.0", dp12),
            ],
            STABLE,
            Err("NoSafePath"),
        ),
    ];
    let mut failures = Vec::new();
    for (name, rels, prefs, want) in cases {
        let cat = catalog(rels);
        let got = Engine::new(&p, &inv, &cat, prefs).propose_conversion();
        let ok = match (&got, &want) {
            (Ok(plan), Ok(w)) => plan
                .install
                .iter()
                .map(|r| r.version.to_string())
                .eq(w.iter().map(|s| s.to_string())),
            (Err(e), Err(w)) => format!("{e:?}").starts_with(w),
            _ => false,
        };
        if !ok {
            failures.push(format!("{name}: want {want:?}, got {got:?}"));
        }
    }
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
