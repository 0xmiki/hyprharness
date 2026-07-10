use hyprharness::{
    Harness,
    models::{KeyModifier, MotionProfile, MouseButton, ScrollDirection},
};

#[tokio::test]
#[ignore = "requires a live Hyprland session"]
async fn observes_live_desktop() {
    let harness = Harness::from_environment(true, None).unwrap();
    let observation = harness.observe_desktop(None).await.unwrap();
    assert!(observation.png.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(observation.metadata.monitor.pixel_width > 0);
    assert!(!harness.list_windows().await.unwrap().windows.is_empty());
    let _ = harness.get_cursor().await.unwrap();
    harness.keyboard_probe().await.unwrap();
    assert!(harness.wait(5).await.unwrap().elapsed_ms >= 5);
}

#[tokio::test]
#[ignore = "moves the pointer in a live Hyprland session"]
async fn moves_and_restores_live_pointer() {
    let harness = Harness::from_environment(false, None).unwrap();
    let focused = harness
        .list_windows()
        .await
        .unwrap()
        .windows
        .into_iter()
        .find(|window| window.focused)
        .unwrap();
    let focus_result = harness
        .focus_window(focused.stable_id.clone())
        .await
        .unwrap();
    assert_eq!(focus_result.focused_window.address, focused.address);
    let origin = harness.get_cursor().await.unwrap().position;
    let monitors = harness.monitors().await.unwrap();
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.contains(&origin))
        .unwrap();
    let offset = if origin.x + 80 < monitor.x + monitor.logical_width() {
        80
    } else {
        -80
    };
    let target = hyprharness::models::Point {
        x: origin.x + offset,
        y: origin.y,
    };
    harness
        .move_pointer(target, None, MotionProfile::Natural)
        .await
        .unwrap();
    harness
        .move_pointer(origin.clone(), None, MotionProfile::Natural)
        .await
        .unwrap();
    assert_eq!(harness.get_cursor().await.unwrap().position, origin);

    if std::env::var("HYPRHARNESS_LIVE_CLICK").as_deref() == Ok("1") {
        harness.click(MouseButton::Left, 1, 120).await.unwrap();
    }
    if std::env::var("HYPRHARNESS_LIVE_SCROLL").as_deref() == Ok("1") {
        harness.scroll(ScrollDirection::Down, 1).await.unwrap();
        harness.scroll(ScrollDirection::Up, 1).await.unwrap();
    }
    if std::env::var("HYPRHARNESS_LIVE_KEYBOARD").as_deref() == Ok("1") {
        let focused = harness
            .list_windows()
            .await
            .unwrap()
            .windows
            .into_iter()
            .find(|window| window.focused)
            .unwrap();
        harness
            .press_key(
                "a".into(),
                vec![KeyModifier::Ctrl],
                1,
                Some(focused.stable_id.clone()),
            )
            .await
            .unwrap();
        harness
            .type_text("hyprharness-live-test".into(), 0, Some(focused.stable_id))
            .await
            .unwrap();
    }
}
