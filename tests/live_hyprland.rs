use hyprharness::{Harness, models::MouseButton};

#[tokio::test]
#[ignore = "requires a live Hyprland session"]
async fn observes_live_desktop() {
    let harness = Harness::from_environment(true, None).unwrap();
    let observation = harness.observe_desktop(None).await.unwrap();
    assert!(observation.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(observation.metadata.monitor.pixel_width > 0);
    assert!(!harness.list_windows().await.unwrap().windows.is_empty());
    let _ = harness.get_cursor().await.unwrap();
}

#[tokio::test]
#[ignore = "moves the pointer in a live Hyprland session"]
async fn moves_and_restores_live_pointer() {
    let harness = Harness::from_environment(false, None).unwrap();
    let origin = harness.get_cursor().await.unwrap().position;
    let monitors = harness.monitors().await.unwrap();
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.contains(&origin))
        .unwrap();
    let target = hyprharness::models::Point {
        x: (origin.x + 10).clamp(monitor.x, monitor.x + monitor.logical_width() - 1),
        y: origin.y,
    };
    harness.move_pointer(target, 50).await.unwrap();
    harness.move_pointer(origin.clone(), 50).await.unwrap();
    assert_eq!(harness.get_cursor().await.unwrap().position, origin);

    if std::env::var("HYPRHARNESS_LIVE_CLICK").as_deref() == Ok("1") {
        harness.click(MouseButton::Left, 1, 120).await.unwrap();
    }
}
