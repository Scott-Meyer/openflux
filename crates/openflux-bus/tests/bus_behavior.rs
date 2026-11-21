use openflux_bus::{
    apply_remote_frame, topic_matches, Broker, FrameLayout, KeyboardGeometry, PublishOptions,
    SubscribeOptions, WireFrame,
};
use serde_json::json;

#[test]
fn hierarchical_wildcards_retained_replay_and_bridge_loop_prevention() {
    assert!(topic_matches("input/#", "input/keys/analog"));
    assert!(topic_matches("input/+/analog", "input/keys/analog"));
    assert!(!topic_matches("input/+/analog", "input/keys/event"));
    assert!(topic_matches("modules/pet/#", "modules/pet"));

    let kb = Broker::new("keyboard");
    let host = Broker::new("host");

    // Retained state published on host replays immediately to a late subscriber.
    host.publish(
        "host/system/metrics",
        json!({ "cpu_percent": 42.0 }),
        PublishOptions::retained(),
    )
    .unwrap();
    let late_sub = host
        .subscribe("host/#", SubscribeOptions::with_retained())
        .unwrap();
    let replayed = late_sub.drain();
    assert_eq!(replayed.len(), 1);
    assert_eq!(replayed[0].topic, "host/system/metrics");

    // Bridging from host -> keyboard preserves origin="host", so a bridge subscriber ignoring
    // origin="host" does not echo the message back to host.
    let kb_outbound = kb
        .subscribe(
            "#",
            SubscribeOptions {
                replay_retained: false,
                ignore_origin: Some("host".into()),
                capacity: 64,
            },
        )
        .unwrap();
    let kb_local_sub = kb.subscribe("#", SubscribeOptions::default()).unwrap();

    let frame = WireFrame::from_bus_message(&replayed[0]);
    apply_remote_frame(&kb, frame, "host").unwrap();

    assert_eq!(kb_local_sub.drain().len(), 1);
    assert!(
        kb_outbound.drain().is_empty(),
        "bridge must not echo remote-origin frames back"
    );
}

#[test]
fn physical_geometry_sdf_distinguishes_keycap_interior_from_trench() {
    let geom = KeyboardGeometry::build(FrameLayout::Ansi);
    assert!(geom.keys.len() >= 84);
    assert_eq!(geom.slots.len(), 4);

    let k = geom.key(49).unwrap(); // 'D' key
    let inside = geom.sample_key_sdf(k.center_x, k.center_y);
    assert!(inside.signed_distance_px < -10.0);
    assert_eq!(inside.nearest_key_index, k.key_index);

    // Midpoint between two adjacent keys on the same row sits in the 2mm trench (> 0).
    let k_next = geom.key(50).unwrap();
    let trench_x = (k.rect.x + k.rect.w + k_next.rect.x) * 0.5;
    let trench = geom.sample_key_sdf(trench_x, k.center_y);
    assert!(
        trench.signed_distance_px > 0.0,
        "2mm inter-key trench must have positive SDF, got {}",
        trench.signed_distance_px
    );
}
