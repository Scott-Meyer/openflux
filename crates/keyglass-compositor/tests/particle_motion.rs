use std::sync::LazyLock;

use keyglass_bus::{
    topics, Broker, FrameLayout, KeyEvent, KeyEventKind, KeyboardGeometry, PublishOptions,
    DISPLAY_HEIGHT_PX, DISPLAY_WIDTH_PX,
};
use keyglass_compositor::{KeySdfGrid, ParticleSystem};
use keyglass_modules::{ModuleInstanceState, ModuleRegistry, ModuleRuntime, ParticleSpawnCmd};

static GEOMETRY: LazyLock<KeyboardGeometry> =
    LazyLock::new(|| KeyboardGeometry::build(FrameLayout::Ansi));
static SDF: LazyLock<KeySdfGrid> = LazyLock::new(|| KeySdfGrid::build(&GEOMETRY));

fn spark(x: f32, y: f32, vx: f32, vy: f32) -> ParticleSpawnCmd {
    ParticleSpawnCmd {
        x,
        y,
        vx,
        vy,
        radius_px: 6.0,
        life_ms: 8_000.0,
        color: [1.0; 4],
        bounce_chance: 0.0,
        lens_refract: 1.55,
        drag: 1.0,
    }
}

fn run_frames(world: &mut ParticleSystem, frames: usize) {
    for _ in 0..frames {
        world.step(1.0 / 60.0, &SDF, &GEOMETRY);
    }
}

#[test]
fn upward_spark_arcs_then_falls() {
    let mut world = ParticleSystem::default();
    world.spawn(&spark(960.0, 160.0, 0.0, -300.0), &SDF);
    run_frames(&mut world, 6);
    let rising = world.particles[0];
    assert!(rising.y < 140.0 && rising.vy > -300.0 && rising.vy < 0.0);

    run_frames(&mut world, 24);
    let falling = world.particles[0];
    assert!(falling.y > 160.0 && falling.vy > 0.0);
}

#[test]
fn key_sides_admit_an_incoming_spark_but_can_ricochet_on_exit() {
    let key = GEOMETRY.key(48).unwrap();
    let right_wall = key.rect.x + key.rect.w;
    let mut entering = spark(key.rect.x - 6.0, key.center_y, 400.0, 0.0);
    entering.bounce_chance = 1.0;
    let mut world = ParticleSystem::default();
    world.spawn(&entering, &SDF);
    run_frames(&mut world, 2);
    let entered = world.particles[0];
    assert!(entered.vx > 0.0 && entered.inside_key && entered.current_scale > 1.0);

    for (wall, direction) in [(key.rect.x, -1.0), (right_wall, 1.0)] {
        for bounce in [0.0, 1.0] {
            let mut shot = spark(wall - 6.0 * direction, key.center_y, 400.0 * direction, 0.0);
            shot.bounce_chance = bounce;
            let mut world = ParticleSystem::default();
            world.spawn(&shot, &SDF);
            run_frames(&mut world, 2);
            let p = world.particles[0];
            assert_eq!(p.vx * direction < 0.0, bounce == 1.0, "{p:?}");
            assert_eq!(p.inside_key, bounce == 1.0);
        }
    }
}

#[test]
fn upward_sparks_clear_key_walls_even_at_full_bounce_chance() {
    let key = GEOMETRY.key(7).unwrap();
    let mut shot = spark(key.center_x, key.rect.y + key.rect.h + 6.0, 0.0, -420.0);
    shot.bounce_chance = 1.0;
    let mut world = ParticleSystem::default();
    world.spawn(&shot, &SDF);
    run_frames(&mut world, 2);
    let rising = world.particles[0];
    assert!(rising.y < key.rect.y + key.rect.h && rising.vy < 0.0 && rising.inside_key);

    let mut exiting = spark(key.center_x, key.rect.y + 6.0, 0.0, -420.0);
    exiting.bounce_chance = 1.0;
    let mut world = ParticleSystem::default();
    world.spawn(&exiting, &SDF);
    run_frames(&mut world, 2);
    let exited = world.particles[0];
    assert!(exited.y < key.rect.y && exited.vy < 0.0 && !exited.inside_key);
}

#[test]
fn falling_spark_enters_freely_bounces_inside_and_eventually_drains() {
    let key = GEOMETRY.key(7).unwrap();
    let mut shot = spark(key.center_x, key.rect.y - 6.0, 0.0, 420.0);
    shot.bounce_chance = 1.0;
    let mut world = ParticleSystem::default();
    world.spawn(&shot, &SDF);
    run_frames(&mut world, 2);
    let entered = world.particles[0];
    assert!(entered.y > key.rect.y && entered.vy > 0.0 && entered.inside_key);

    let mut inner_bounce = false;
    for _ in 0..60 {
        run_frames(&mut world, 1);
        let p = world.particles[0];
        inner_bounce |=
            p.vy < 0.0 && p.inside_key && p.y > key.center_y && p.y < key.rect.y + key.rect.h;
    }
    assert!(inner_bounce, "the inside bottom wall should ricochet");
    run_frames(&mut world, 240);
    let p = world.particles[0];
    assert!(
        p.y > 1020.0,
        "even a spark with guaranteed hard bounces should drain: {p:?}"
    );
}

#[test]
fn fast_sparks_bounce_instead_of_crossing_between_frames() {
    let mut world = ParticleSystem::default();
    world.spawn(&spark(950.0, 160.0, 800.0, 0.0), &SDF);
    world.spawn(&spark(970.0, 160.0, -800.0, 0.0), &SDF);
    world.step(1.0 / 30.0, &SDF, &GEOMETRY);

    let [left, right] = &world.particles[..] else {
        panic!("both sparks should survive the collision");
    };
    assert!(left.x + left.radius_px <= right.x - right.radius_px);
    assert!(left.vx < 0.0 && right.vx > 0.0);
}

#[test]
fn sparks_stack_on_the_floor_and_fade_when_stranded() {
    let mut world = ParticleSystem::default();
    world.spawn(&spark(960.0, 1020.0, 0.0, 0.0), &SDF);
    world.spawn(&spark(960.0, 990.0, 0.0, 0.0), &SDF);
    run_frames(&mut world, 180);

    let [bottom, top] = &world.particles[..] else {
        panic!("both sparks should still be glowing");
    };
    assert!(bottom.y > 1020.0 && bottom.y + bottom.radius_px < 1050.0);
    assert!(top.y + top.radius_px <= bottom.y - bottom.radius_px + 0.2);
    assert!(bottom.vy.abs() < 5.0 && top.vy.abs() < 5.0);
    let resting_opacity = bottom.dot().color[3];

    run_frames(&mut world, 270);
    assert!(world.particles[0].dot().color[3] < resting_opacity * 0.5);
    run_frames(&mut world, 45);
    assert!(world.particles.is_empty());
}

#[test]
fn sparks_roll_out_both_open_sides_without_waiting_for_expiry() {
    let mut world = ParticleSystem::default();
    let width = DISPLAY_WIDTH_PX as f32;
    world.spawn(&spark(24.0, 1070.0, 0.0, 0.0), &SDF);
    world.spawn(&spark(width - 24.0, 1070.0, 0.0, 0.0), &SDF);
    run_frames(&mut world, 48);
    assert!(world.particles[0].x < 20.0 && world.particles[1].x > width - 20.0);
    run_frames(&mut world, 110);
    assert!(world.particles.is_empty());
}

#[test]
fn low_key_walls_leave_the_drain_open() {
    for row in [4, 5] {
        let key = GEOMETRY
            .keys
            .iter()
            .find(|k| k.row == row && k.col == 0)
            .unwrap();
        let right = key.rect.x + key.rect.w;
        let bottom = key.rect.y + key.rect.h;
        let mut side = spark(right - 6.0, key.center_y, 400.0, 0.0);
        side.bounce_chance = 1.0;
        let mut sideways = ParticleSystem::default();
        sideways.spawn(&side, &SDF);
        run_frames(&mut sideways, 2);
        assert!(sideways.particles[0].x > right && sideways.particles[0].vx > 0.0);

        let mut falling = ParticleSystem::default();
        let mut shot = spark(key.center_x, bottom - 6.0, 0.0, 420.0);
        shot.bounce_chance = 1.0;
        falling.spawn(&shot, &SDF);
        run_frames(&mut falling, 2);
        let p = falling.particles[0];
        assert!(p.y > bottom && p.vy > 0.0, "row {row}: {p:?}");
    }
}

#[test]
fn floor_rebound_no_longer_interacts_with_keys() {
    let key = GEOMETRY
        .keys
        .iter()
        .find(|k| k.row == 5 && k.col == 4)
        .unwrap();
    let mut shot = spark(key.center_x, DISPLAY_HEIGHT_PX as f32 - 28.0, 0.0, 2200.0);
    shot.bounce_chance = 1.0;
    let mut world = ParticleSystem::default();
    world.spawn(&shot, &SDF);
    run_frames(&mut world, 5);
    let rebound = world.particles[0];
    assert!(rebound.y < DISPLAY_HEIGHT_PX as f32 - 36.0 && rebound.vy < 0.0);
    assert!(!rebound.inside_key);
}

fn keypress_runtime() -> (Broker, ModuleRuntime, KeyEvent) {
    use std::collections::BTreeMap;
    use std::path::PathBuf;

    let modules = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../modules");
    let registry = ModuleRegistry::new(
        modules.clone(),
        modules.join(".test-no-installed-modules"),
        modules.join(".test-no-local-modules"),
        modules.join(".test-no-state"),
    );
    let broker = Broker::new("particle-motion-test");
    let mut runtime = ModuleRuntime::new(broker.clone(), GEOMETRY.clone());
    runtime.sync_modules(
        &registry.discover(),
        &BTreeMap::from([(
            "fx-key-particles".into(),
            ModuleInstanceState {
                enabled: true,
                ..Default::default()
            },
        )]),
    );
    let key = GEOMETRY.key(48).unwrap();
    let event = KeyEvent {
        key_index: key.key_index,
        kind: KeyEventKind::Press,
        timestamp_ms: 0,
        x_px: key.center_x,
        y_px: key.center_y,
        w_px: key.rect.w,
        h_px: key.rect.h,
        uv_x: key.uv_x,
        uv_y: key.uv_y,
        row: key.row,
        col: key.col,
        depth_um: 2200,
        velocity_um_s: 14000,
        color: None,
    };
    (broker, runtime, event)
}

#[test]
fn bundled_keypress_bursts_vary_around_the_upward_bias() {
    use std::collections::BTreeSet;

    let (broker, mut runtime, mut event) = keypress_runtime();
    let mut counts = BTreeSet::new();
    let mut colors = BTreeSet::new();
    let mut radii = BTreeSet::new();
    let mut up = 0;
    let mut down = 0;
    let mut total = 0;
    let mut fastest_rise: f32 = 0.0;
    let presses = 256;
    for press in 0..presses {
        event.timestamp_ms = press * 17;
        broker
            .publish_typed(topics::INPUT_KEYS_EVENT, &event, PublishOptions::default())
            .unwrap();
        let spawns = runtime
            .step_frame(1.0 / 60.0, event.timestamp_ms)
            .particle_spawns;
        assert!((2..=5).contains(&spawns.len()));
        total += spawns.len();
        counts.insert(spawns.len());
        for p in spawns {
            up += usize::from(-p.vy > p.vx.abs());
            down += usize::from(p.vy > p.vx.abs());
            fastest_rise = fastest_rise.max(-p.vy);
            assert!(p.vx.abs() < 460.0 && p.vy < 460.0);
            colors.insert(p.color.map(f32::to_bits));
            assert!((7.0..=10.0).contains(&p.radius_px));
            radii.insert(p.radius_px.to_bits());
            assert_eq!(p.bounce_chance, 0.8);
        }
    }
    assert!((3.1..3.9).contains(&(total as f32 / presses as f32)) && counts.len() > 1);
    assert!((0.64..0.81).contains(&(up as f32 / total as f32)));
    assert!(fastest_rise > 600.0);
    assert!((0.05..0.15).contains(&(down as f32 / total as f32)));
    assert_eq!(colors.len(), 10);
    assert!(radii.len() > 4);
}

#[test]
fn held_keys_emit_small_follow_up_bursts_until_release() {
    let (broker, mut runtime, mut event) = keypress_runtime();
    broker
        .publish_typed(topics::INPUT_KEYS_EVENT, &event, PublishOptions::default())
        .unwrap();
    let mut step = |now_ms| runtime.step_frame(1.0 / 60.0, now_ms).particle_spawns.len();
    assert!((2..=5).contains(&step(0)));
    assert_eq!(step(349), 0);
    assert!((1..=2).contains(&step(350)));
    assert_eq!(step(774), 0);
    assert!((1..=2).contains(&step(850)));

    event.kind = KeyEventKind::Release;
    event.timestamp_ms = 950;
    broker
        .publish_typed(topics::INPUT_KEYS_EVENT, &event, PublishOptions::default())
        .unwrap();
    assert_eq!(step(950), 0);
    assert_eq!(step(1400), 0);
}

#[test]
fn keypress_bursts_crest_two_to_three_rows_above_the_key() {
    let (broker, mut runtime, mut event) = keypress_runtime();
    let next_row = GEOMETRY
        .keys
        .iter()
        .find(|k| k.row == event.row + 1)
        .unwrap();
    let row_pitch = next_row.center_y - event.y_px;
    let mut rises = Vec::new();
    for press in 0..48 {
        event.timestamp_ms = press * 17;
        broker
            .publish_typed(topics::INPUT_KEYS_EVENT, &event, PublishOptions::default())
            .unwrap();
        let mut world = ParticleSystem::default();
        for p in runtime
            .step_frame(1.0 / 60.0, event.timestamp_ms)
            .particle_spawns
        {
            world.spawn(&p, &SDF);
        }
        let mut highest = event.y_px;
        for _ in 0..55 {
            run_frames(&mut world, 1);
            for p in &world.particles {
                highest = highest.min(p.y);
            }
        }
        rises.push((event.y_px - highest) / row_pitch);
    }
    rises.sort_by(f32::total_cmp);
    let median = rises[rises.len() / 2];
    assert!(
        (2.0..=3.2).contains(&median),
        "median apex: {median:.2} rows"
    );
    let clear_hands = rises.iter().filter(|&&rows| rows >= 1.9).count();
    assert!(
        clear_hands >= 40,
        "only {clear_hands} of 48 presses cleared two rows"
    );
}
