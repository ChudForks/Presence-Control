//! Drive real pointer events through the production controls, including focus changes.
use super::{model, profile_form, MainWindow, ProfileRow};
use presence_core::model::default_config;
use slint::{
    platform::{
        software_renderer::{MinimalSoftwareWindow, RepaintBufferType},
        Key, Platform, PlatformError, PointerEventButton, WindowAdapter, WindowEvent,
    },
    ComponentHandle, LogicalPosition, PhysicalSize, Rgb8Pixel, SharedString,
};
use std::{cell::RefCell, rc::Rc};

slint::slint! {
    import { UiButton, IconButton, Toggle, Choice, Input } from "../ui/components.slint";

    export component ControlsTest inherits Window {
        width: 400px;
        height: 300px;
        in-out property <int> button-clicks;
        in-out property <int> icon-clicks;
        in-out property <bool> checked;
        in-out property <int> choices;
        in-out property <int> choice-index;
        in-out property <string> input;
        UiButton { x: 10px; y: 10px; text: "Action"; clicked => { root.button-clicks += 1; } }
        IconButton { x: 200px; y: 10px; icon: "copy"; clicked => { root.icon-clicks += 1; } }
        Toggle { x: 10px; y: 80px; checked: root.checked; toggled => { root.checked = !root.checked; } }
        Choice { x: 10px; y: 120px; width: 180px; options: ["First", "Second"]; current-index <=> root.choice-index; selected(_) => { root.choices += 1; } }
        Input { x: 10px; y: 180px; width: 200px; text <=> root.input; }
        UiButton { x: 10px; y: 240px; text: "Disabled"; enabled: false; clicked => { root.button-clicks += 100; } }
    }
}

struct TestPlatform(Rc<MinimalSoftwareWindow>);

impl Platform for TestPlatform {
    fn create_window_adapter(&self) -> Result<Rc<dyn WindowAdapter>, PlatformError> {
        Ok(self.0.clone())
    }
}

fn draw(window: &MinimalSoftwareWindow) {
    let size = window.size();
    window.draw_if_needed(|renderer| {
        let mut pixels = vec![Rgb8Pixel::default(); size.width as usize * size.height as usize];
        renderer.render(&mut pixels, size.width as usize);
    });
}

fn click(window: &slint::Window, x: f32, y: f32) {
    let position = LogicalPosition::new(x, y);
    window.dispatch_event(WindowEvent::PointerMoved { position });
    window.dispatch_event(WindowEvent::PointerPressed {
        position,
        button: PointerEventButton::Left,
    });
    window.dispatch_event(WindowEvent::PointerReleased {
        position,
        button: PointerEventButton::Left,
    });
}

fn key(window: &slint::Window, text: SharedString) {
    window.dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
    window.dispatch_event(WindowEvent::KeyReleased { text });
}

#[test]
fn first_click_activates_controls_and_transfers_keyboard_focus() {
    let window = MinimalSoftwareWindow::new(RepaintBufferType::ReusedBuffer);
    slint::platform::set_platform(Box::new(TestPlatform(window.clone()))).unwrap();
    let controls = ControlsTest::new().unwrap();
    window.set_size(PhysicalSize::new(400, 300));
    controls.show().unwrap();
    window.dispatch_event(WindowEvent::WindowActiveChanged(true));
    draw(&window);

    click(&window, 40.0, 30.0);
    assert_eq!(
        controls.get_button_clicks(),
        1,
        "first button click activates"
    );
    key(&window, Key::Space.into());
    assert_eq!(
        controls.get_button_clicks(),
        2,
        "click also gives keyboard focus"
    );
    click(&window, 216.0, 26.0);
    assert_eq!(controls.get_icon_clicks(), 1, "first icon click activates");
    key(&window, Key::Return.into());
    assert_eq!(
        controls.get_icon_clicks(),
        2,
        "Enter activates the focused icon"
    );
    click(&window, 27.0, 90.0);
    assert!(
        controls.get_checked(),
        "first toggle click changes its value"
    );
    key(&window, Key::Space.into());
    assert!(!controls.get_checked(), "toggle retains keyboard control");

    click(&window, 50.0, 140.0);
    draw(&window);
    click(&window, 50.0, 226.0);
    assert_eq!(
        controls.get_choices(),
        1,
        "first dropdown and option clicks select"
    );
    assert_eq!(controls.get_choice_index(), 1);
    click(&window, 30.0, 200.0);
    key(&window, "X".into());
    assert_eq!(controls.get_input(), "X", "first input click allows typing");
    click(&window, 40.0, 260.0);
    assert_eq!(
        controls.get_button_clicks(),
        2,
        "disabled buttons stay inactive"
    );
    controls.hide().unwrap();
    drop(controls);

    let ui = MainWindow::new().unwrap();
    let config = default_config();
    ui.set_form(profile_form(&config.profiles[0]));
    ui.set_profiles(model(
        config
            .profiles
            .iter()
            .map(|p| ProfileRow {
                id: p.id.clone().into(),
                title: p.name.clone().into(),
                selected: p.id == config.profiles[0].id,
                ..Default::default()
            })
            .collect(),
    ));
    ui.set_profile_names(model(
        config
            .profiles
            .iter()
            .map(|p| p.name.clone().into())
            .collect(),
    ));
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    ui.on_action(move |action, argument| recorded.borrow_mut().push((action, argument)));
    window.set_size(PhysicalSize::new(1470, 1000));
    ui.show().unwrap();
    draw(&window);

    for (y, page) in [(214.0, 1), (264.0, 2), (956.0, 4), (165.0, 0)] {
        click(&window, 100.0, y);
        assert_eq!(
            ui.get_page(),
            page,
            "first navigation click changes the page"
        );
        draw(&window);
    }
    click(&window, 800.0, 280.0);
    assert_eq!(
        actions.borrow().last().unwrap().1,
        "deep-work",
        "first card click selects the presence"
    );
    for (x, tab) in [(405.0, 1), (490.0, 2), (580.0, 3), (315.0, 0)] {
        click(&window, x, 460.0);
        assert_eq!(
            ui.get_editor_tab(),
            tab,
            "first editor-tab click switches tabs"
        );
        draw(&window);
    }
    key(&window, Key::RightArrow.into());
    assert_eq!(
        ui.get_editor_tab(),
        1,
        "clicked tabs retain arrow-key navigation"
    );
    ui.set_modal("delete-profile".into());
    draw(&window);
    click(&window, 100.0, 214.0);
    assert_eq!(ui.get_page(), 0, "dialogs still block background clicks");
    ui.hide().unwrap();
}
