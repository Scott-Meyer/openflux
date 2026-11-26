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

#[test]
fn mux_framing_and_subscription_driven_broker_mirroring() {
    use openflux_bus::{
        decode_media_payload, encode_media_payload, BrokerMirror, MediaCodec, MuxChannel,
        MuxDecoder, MuxFrame, FLAG_KEYFRAME,
    };

    // 1. Priority-multiplexed framing survives arbitrary byte-splitting & coalescing
    let media_payload =
        encode_media_payload(MediaCodec::H264AnnexB, "bg-stream", &[0, 0, 0, 1, 0x67, 0x42]);
    let f1 = MuxFrame::new(MuxChannel::Bus, 0, 0, b"{\"type\":\"ping\",\"ts_ms\":1}".to_vec());
    let f2 = MuxFrame::new(MuxChannel::Media, FLAG_KEYFRAME, 7, media_payload);
    let mut wire = Vec::new();
    wire.extend_from_slice(&f1.encode());
    wire.extend_from_slice(&f2.encode());

    let mut dec = MuxDecoder::new();
    let mut decoded = Vec::new();
    for ch in wire.chunks(3) {
        decoded.extend(dec.feed(ch));
    }
    assert_eq!(decoded.len(), 2);
    assert_eq!(decoded[0], f1);
    assert_eq!(decoded[1], f2);
    let (codec, name, bytes) = decode_media_payload(&decoded[1].payload).unwrap();
    assert_eq!(codec, MediaCodec::H264AnnexB);
    assert_eq!(name, "bg-stream");
    assert_eq!(bytes, &[0, 0, 0, 1, 0x67, 0x42]);

    // 2. Subscription-driven BrokerMirror between host and keyboard (retained replay, loop-free, LWT!)
    let host = Broker::new("host");
    let kb = Broker::new("keyboard");

    // Pre-populate retained state on host before keyboard subscribes
    host.publish(
        "host/app/active",
        json!({"app_id": "com.tinyspeck.slackmacgap"}),
        PublishOptions::retained(),
    )
    .unwrap();
    host.publish(
        "host/unsubscribed/secret",
        json!({"x": 1}),
        PublishOptions::retained(),
    )
    .unwrap();

    // Keyboard module subscribes ONLY to `host/app/#`
    let _kb_mod_sub = kb
        .subscribe("host/app/#", SubscribeOptions::default())
        .unwrap();
    // Host module subscribes to `modules/demo/event` (and NOT `modules/demo/highfreq`)
    let _host_mod_sub = host
        .subscribe("modules/demo/event", SubscribeOptions::default())
        .unwrap();

    let mut host_mirror = BrokerMirror::new(host.clone(), "keyboard");
    let mut kb_mirror = BrokerMirror::new(kb.clone(), "host");

    // Exchange Subscribe advertisements
    let kb_sub_frame = kb_mirror.build_subscribe_if_changed(&[], true).unwrap();
    let host_replays = host_mirror.on_remote_frame(kb_sub_frame);
    assert_eq!(
        host_replays.len(),
        1,
        "host replays ONLY retained matching host/app/#"
    );
    for pf in host_replays {
        kb_mirror.on_remote_frame(pf);
    }
    assert_eq!(
        kb.get_retained("host/app/active").unwrap().payload["app_id"],
        "com.tinyspeck.slackmacgap"
    );
    assert!(kb.get_retained("host/unsubscribed/secret").is_none());

    let host_sub_frame = host_mirror.build_subscribe_if_changed(&[], true).unwrap();
    kb_mirror.on_remote_frame(host_sub_frame);

    // Keyboard publishes both subscribed event and unsubscribed high-frequency telemetry
    kb.publish(
        "modules/demo/event",
        json!({"ok": true}),
        PublishOptions::default(),
    )
    .unwrap();
    kb.publish(
        "modules/demo/highfreq",
        json!({"hz": 120}),
        PublishOptions::default(),
    )
    .unwrap();
    let kb_out = kb_mirror.drain_outgoing();
    assert_eq!(
        kb_out.len(),
        1,
        "unsubscribed high-frequency telemetry must NOT cross USB"
    );
    for pf in kb_out {
        host_mirror.on_remote_frame(pf);
    }
    // Loop immunity: host_mirror must NOT echo keyboard's frame back!
    assert!(host_mirror.drain_outgoing().is_empty());

    // Security invariant: raw keystrokes (`input/keys/*`) must NEVER cross BrokerMirror even if a
    // peer explicitly subscribes to `#` or `input/#`!
    kb_mirror.on_remote_frame(openflux_bus::PeerBusFrame::Subscribe {
        node: "host".into(),
        session_id: 999,
        patterns: vec!["#".into(), "input/#".into(), "input/keys/event".into()],
    });
    kb.publish("input/keys/event", json!({"key_index": 60}), PublishOptions::default())
        .unwrap();
    kb.publish("input/keys/analog", json!({"depth_um": 1200}), PublishOptions::default())
        .unwrap();
    assert!(
        kb_mirror.drain_outgoing().is_empty(),
        "raw keystrokes and analog depths must NEVER be exported across BrokerMirror"
    );

    // Disconnect purges peer-owned retained state cleanly (LWT)
    kb_mirror.on_disconnect();
    assert!(kb.get_retained("host/app/active").is_none());

    // 3. Independent peer restart mid-session: keyboard crashes & creates a brand-new BrokerMirror
    //    while host stays running. Keyboard sends its Subscribe(new session_id) -> host MUST
    //    automatically reply with its own Subscribe + retained replay (zero ping-pong, full bidirectional recovery!).
    let kb2 = Broker::new("keyboard");
    let _kb2_sub = kb2
        .subscribe("host/app/#", SubscribeOptions::default())
        .unwrap();
    let mut kb2_mirror = BrokerMirror::new(kb2.clone(), "host");
    let kb2_sub_f = kb2_mirror.build_subscribe_if_changed(&[], true).unwrap();

    let host_replies = host_mirror.on_remote_frame(kb2_sub_f);
    // Must contain BOTH host's own Subscribe back AND the retained host/app/active replay!
    assert!(
        host_replies.iter().any(|f| matches!(f, openflux_bus::PeerBusFrame::Subscribe { .. })),
        "host must re-advertise its own subscriptions when a peer restarts with a new session_id"
    );
    let mut kb2_followups = Vec::new();
    for rf in host_replies {
        kb2_followups.extend(kb2_mirror.on_remote_frame(rf));
    }
    assert_eq!(
        kb2.get_retained("host/app/active").unwrap().payload["app_id"],
        "com.tinyspeck.slackmacgap"
    );
    // Feed followups back to host and confirm convergence terminates (no infinite Subscribe ping-pong!)
    for f in kb2_followups {
        let more = host_mirror.on_remote_frame(f);
        assert!(
            !more.iter().any(|x| matches!(x, openflux_bus::PeerBusFrame::Subscribe { .. })),
            "Subscribe exchange must converge cleanly in 1 RTT without ping-pong"
        );
    }
    // Verify keyboard -> host direction is ALREADY restored after keyboard-only restart!
    kb2.publish("modules/demo/event", json!({"restored": true}), PublishOptions::default())
        .unwrap();
    assert_eq!(
        kb2_mirror.drain_outgoing().len(),
        1,
        "restarted keyboard must immediately mirror host-subscribed topics back to host"
    );
}
