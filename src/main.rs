use serde::Deserialize;
use std::ffi::OsStr;
use std::io::BufWriter;
use std::io::copy;
use std::os::windows::ffi::OsStrExt;
use tempfile::Builder;
use tray_icon::Icon;
use tray_icon::TrayIcon;
use tray_icon::TrayIconBuilder;
use tray_icon::TrayIconEvent;
use tray_icon::menu::Menu;
use tray_icon::menu::MenuEvent;
use tray_icon::menu::MenuId;
use tray_icon::menu::MenuItem;
use windows::Win32::UI::WindowsAndMessaging::SPI_SETDESKWALLPAPER;
use windows::Win32::UI::WindowsAndMessaging::SPIF_SENDCHANGE;
use windows::Win32::UI::WindowsAndMessaging::SPIF_UPDATEINIFILE;
use windows::Win32::UI::WindowsAndMessaging::SystemParametersInfoW;
use winit::application::ApplicationHandler;
use winit::event_loop::EventLoop;

fn main() {
    let event_loop = EventLoop::<UserEvent>::with_user_event().build().unwrap();

    let proxy = event_loop.create_proxy();
    TrayIconEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::TrayIconEvent(event)).unwrap();
    }));
    let proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |event| {
        proxy.send_event(UserEvent::MenuEvent(event)).unwrap();
    }));

    let mut app = Application::new();

    event_loop.run_app(&mut app).unwrap();
}

#[derive(Debug)]
enum UserEvent {
    TrayIconEvent(tray_icon::TrayIconEvent),
    MenuEvent(tray_icon::menu::MenuEvent),
}

struct Application {
    tray_icon: Option<TrayIcon>,
    menu_item_update: Option<MenuId>,
    menu_item_exit: Option<MenuId>,
}

impl Application {
    fn new() -> Application {
        Application {
            tray_icon: None,
            menu_item_update: None,
            menu_item_exit: None,
        }
    }

    fn new_tray_icon(&mut self) -> TrayIcon {
        TrayIconBuilder::new()
            .with_menu(Box::new(Self::new_tray_menu(self)))
            .with_tooltip("BingWallpaper")
            .with_icon(Self::load_icon())
            .with_title("x")
            .build()
            .unwrap()
    }

    fn new_tray_menu(&mut self) -> Menu {
        let menu = Menu::new();

        let menu_item_update = MenuItem::new("更新壁纸", true, None);
        menu.append(&menu_item_update).unwrap();
        self.menu_item_update = Some(menu_item_update.into_id());

        let menu_item_exit = MenuItem::new("退出", true, None);
        menu.append(&menu_item_exit).unwrap();
        self.menu_item_exit = Some(menu_item_exit.into_id());

        menu
    }

    fn load_icon() -> Icon {
        Icon::from_path(
            "D:\\OneDrive\\Gits\\Rust\\BingWallpaper\\assets\\favicon.ico",
            None,
        )
        .unwrap()
    }
}

impl ApplicationHandler<UserEvent> for Application {
    fn resumed(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop) {}

    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }

    fn new_events(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        cause: winit::event::StartCause,
    ) {
        if winit::event::StartCause::Init == cause {
            self.tray_icon = Some(Self::new_tray_icon(self));
        }
    }

    fn user_event(&mut self, _event_loop: &winit::event_loop::ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::TrayIconEvent(_tray_icon_event) => {}
            UserEvent::MenuEvent(menu_event) => {
                if &menu_event.id == self.menu_item_update.as_ref().unwrap() {
                    handle_update_wallpaper();
                    return;
                }

                if &menu_event.id == self.menu_item_exit.as_ref().unwrap() {
                    std::process::exit(0);
                }
            }
        };
    }
}

#[derive(Deserialize)]
struct HpImage {
    url: String,
}

#[derive(Deserialize)]
struct HpJson {
    images: Vec<HpImage>,
}

fn handle_update_wallpaper() {
    std::thread::spawn(move || {
        let hp_url = "https://cn.bing.com/HPImageArchive.aspx?format=js&idx=0&n=1&mkt=zh-CN";
        let hp_response = match reqwest::blocking::get(hp_url) {
            Ok(it) => it,
            Err(_) => return,
        };
        let hp_json = match hp_response.json::<HpJson>() {
            Ok(it) => it,
            Err(_) => return,
        };
        let image_json = match hp_json.images.get(0) {
            Some(it) => it,
            None => return,
        };
        let image_url = &image_json.url;
        let image_url = format!("https://s.cn.bing.net{}", image_url);
        let image_response = match reqwest::blocking::get(&image_url) {
            Ok(it) => it,
            Err(_) => return,
        };

        let to_file = match Builder::new().keep(true).suffix(".jpg").tempfile() {
            Ok(it) => it,
            Err(_) => return,
        };
        let mut to_file_writer = BufWriter::new(&to_file);
        let mut image_response_reader = image_response;

        if let Err(_) = copy(&mut image_response_reader, &mut to_file_writer) {
            return;
        }

        let to_path = to_file.path().display().to_string();
        drop(to_file_writer);
        drop(to_file);

        if let Err(_) = set_wallpaper(&to_path) {
            return;
        }

        println!("{:?}", &to_path);
    });
}

fn set_wallpaper(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let wide: Vec<u16> = OsStr::new(path)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    unsafe {
        SystemParametersInfoW(
            SPI_SETDESKWALLPAPER,
            0,
            Some(wide.as_ptr() as _),
            SPIF_UPDATEINIFILE | SPIF_SENDCHANGE,
        )?;
    }

    Ok(())
}
