use windows::Win32::UI::Input::KeyboardAndMouse::*;
use anyhow::Result;
use std::mem::size_of;

pub fn inject_mouse_move(x: i32, y: i32) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: x,
                dy: y,
                mouseData: 0,
                dwFlags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE,
                time: 0,
                dwExtraInfo: 0,
            }
        }
    };

    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };

    Ok(())
}

pub fn inject_mouse_button(button: &altreach_proto::MouseButton, pressed: bool) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                mouseData: 0,
                dwFlags: match button {
                    altreach_proto::MouseButton::Left =>  if pressed { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP },
                    altreach_proto::MouseButton::Right => if pressed { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP },
                    altreach_proto::MouseButton::Middle => if pressed { MOUSEEVENTF_MIDDLEDOWN } else { MOUSEEVENTF_MIDDLEUP },
                },
                time: 0,
                dwExtraInfo: 0,
            }
        }
    };

    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };

    Ok(())
}
pub fn inject_key(vk_code: u16, pressed: bool) -> Result<()> {
    let input = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: VIRTUAL_KEY(vk_code),
                wScan: 0,
                dwFlags: if pressed { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP },
                time: 0,
                dwExtraInfo: 0,
            }
        }
    };

    unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };

    Ok(())
}

pub fn inject_mouse_scroll(delta_x: i32, delta_y: i32) -> Result<()> {
    if delta_y != 0 {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (delta_y * 10) as u32,
                    dwFlags: MOUSEEVENTF_WHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                }
            }
        };

        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }
    if delta_x != 0 {
        let input = INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx: 0,
                    dy: 0,
                    mouseData: (delta_x * 10) as u32,
                    dwFlags: MOUSEEVENTF_HWHEEL,
                    time: 0,
                    dwExtraInfo: 0,
                }
            }
        };

        unsafe { SendInput(&[input], size_of::<INPUT>() as i32) };
    }

    Ok(())
}
