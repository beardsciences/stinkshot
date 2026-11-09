//! English strings for the shell group: shell.rs, shell/, tray.rs, toast.rs, pin.rs,
//! record_ui.rs, errors.rs, actions.rs, cli.rs output and the rest of crates/app.
//! Keys start with `shell.`.

pub fn get(key: &str) -> Option<&'static str> {
    Some(match key {
        "shell.tray.history" => "History",
        "shell.tray.settings" => "Settings",
        "shell.tray.quit" => "Quit",
        "shell.action.region" => "Region",
        "shell.action.window" => "Window",
        "shell.action.fullscreen" => "Full Screen",
        "shell.action.scrolling" => "Scrolling",
        "shell.action.ocr" => "Copy Text (OCR)",
        "shell.action.repeat_last" => "Repeat Last Region",
        "shell.action.pick_color" => "Pick Color",
        "shell.action.record_mp4" => "Record MP4",
        "shell.action.record_gif" => "Record GIF",
        "shell.action.region.desc" => "Freeze the screen and drag a region.",
        "shell.action.window.desc" => "Capture the active window with rounded corners and alpha.",
        "shell.action.fullscreen.desc" => "Capture the monitor under the cursor.",
        "shell.action.scrolling.desc" => "Select a region, Stinkshot scrolls and stitches it.",
        "shell.action.ocr.desc" => "Select a region and copy the recognized text.",
        "shell.action.repeat_last.desc" => "Capture the previous region again without selecting.",
        "shell.action.pick_color.desc" => "Click any pixel to copy its color as hex, RGB or HSL.",
        "shell.action.record_mp4.desc" => "Select a region and record with hardware encoding.",
        "shell.action.record_gif.desc" => "Select a region and record a size-optimized GIF.",
        "shell.error.disk_full" => "The disk is full. Free up some space, then try again.",
        "shell.error.access_denied" => {
            "Windows blocked access to the file or folder. Check that it is not read-only or protected, or choose another folder."
        }
        "shell.error.path_too_long" => {
            "The file path is too long. Choose a shorter folder or file name pattern."
        }
        "shell.error.not_found" => {
            "The file or folder could not be found. It may have been moved or deleted."
        }
        "shell.error.in_use" => "Another program is using the file. Close it, then try again.",
        "shell.error.network_path" => {
            "The network location is not reachable. Check the connection, then try again."
        }
        "shell.error.clipboard_busy" => {
            "Another app is using the clipboard. Try again in a moment."
        }
        "shell.error.offline" => {
            "Stinkshot could not reach the server. Check your internet connection and try again later."
        }
        "shell.toast.edit" => "Edit",
        "shell.toast.copy" => "Copy",
        "shell.toast.pin" => "Pin",
        "shell.toast.copy_text" => "Copy Text",
        "shell.toast.folder" => "Folder",
        "shell.toast.play" => "Play",
        "shell.toast.edit.tip" => "Open in the Editor",
        "shell.toast.copy_file.tip" => "Copy the File to the Clipboard",
        "shell.toast.copy_image.tip" => "Copy the Image to the Clipboard",
        "shell.toast.pin.tip" => "Pin to Screen",
        "shell.toast.copy_text.tip" => "Copy Recognized Text",
        "shell.toast.folder.tip" => "Show in Folder",
        "shell.toast.dismiss" => "Dismiss",
        "shell.toast.ocr_missing" => "Copy Text Needs an OCR Language in Windows Settings",
        "shell.tray.update_check_on" => "Update Check On",
        "shell.tray.offline" => "Offline",
        "shell.pin.copied" => "Copied",
        "shell.pin.copy_failed" => "Could Not Copy",
        "shell.pin.copy_image" => "Copy Image",
        "shell.pin.open_editor" => "Open in Editor",
        "shell.pin.reset_zoom" => "Reset Zoom to 100%",
        "shell.pin.close" => "Close Pin",
        "shell.pin.opacity.tip" => "Opacity (Ctrl+scroll)",
        "shell.pin.copy.tip" => "Copy (Ctrl+C)",
        "shell.pin.close.tip" => "Close Pin (Esc)",
        "shell.notice.no_last_region" => "No Region Remembered Yet: Select One",
        "shell.notice.last_region_off_screen" => "Last Region Is Off Screen: Select a New One",
        "shell.error.window_capture" => {
            "Windows could not capture that window. Try again, or use Region capture."
        }
        "shell.start.retry" => "Try starting it again.",
        "shell.welcome.no_key" => {
            "Capture from the Stinkshot tray icon. Start Stinkshot again to open History."
        }
        "shell.hud.mic_system" => "Mic + System",
        "shell.hud.mic" => "Mic",
        "shell.hud.system_audio" => "System Audio",
        "shell.hud.no_audio" => "No Audio",
        "shell.hotkeys.inactive" => "Some Hotkeys Are Not Active",
        "shell.tray.unavailable" => "Tray Icon Unavailable",
        "shell.tray.unavailable.body" => {
            "Stinkshot is running but could not create its tray icon. Use the hotkeys or the command line."
        }
        "shell.welcome.title" => "Stinkshot Is Running",
        "shell.star.failed.body" => {
            "The history index could not be updated. Try again in a moment."
        }
        "shell.star.failed" => "Could Not Update Star",
        "shell.busy.recording" => "Recording in Progress",
        "shell.busy.recording.capture" => "Stop or finish the recording before taking a capture.",
        "shell.busy.capture_open" => "Finish or cancel the capture that is already open first.",
        "shell.busy.capture" => "Finish the current capture first.",
        "shell.busy.recording.edit" => "Stop the recording before editing a capture.",
        "shell.capture.no_image.body" => {
            "Windows did not hand over the screen image. Try again; if it keeps failing, restart Stinkshot."
        }
        "shell.capture.screen_failed" => "Could Not Capture the Screen",
        "shell.capture.no_window" => "No Window to Capture",
        "shell.capture.no_window.body" => {
            "Click the window you want to capture first, then try again."
        }
        "shell.capture.no_monitor.body" => {
            "The monitor under the cursor was not in the screen image. Move the cursor to the monitor you want and try again."
        }
        "shell.ocr.no_text" => "No Text Found",
        "shell.ocr.empty" => "The selection was empty.",
        "shell.selection.lost" => "Could Not Read the Selection",
        "shell.selection.lost.body" => "The screen snapshot was lost; try again.",
        "shell.selection.too_small" => "Selection Too Small",
        "shell.selection.too_small.scroll" => "Select a larger area to scroll-capture.",
        "shell.selection.too_small.record" => "Select a larger area to record.",
        "shell.busy.recording.record" => "Finish the current recording first.",
        "shell.record.temp_failed.body" => {
            "The history folder could not be prepared for the recording. Check the folder in Settings > History."
        }
        "shell.record.start_failed" => "Could Not Start Recording",
        "shell.record.problem.body" => "The recorder hit a problem.",
        "shell.record.problem.tail" => "Stopping and keeping what was recorded.",
        "shell.record.problem" => "Recording Problem",
        "shell.capture.window_failed" => "Could Not Capture the Window",
        "shell.edit.not_yet" => "Cannot Edit Yet",
        "shell.scroll.stopped" => "Scrolling Capture Stopped Early",
        "shell.scroll.stopped.body" => {
            "The page could not be matched any further; kept what was captured."
        }
        "shell.scroll.failed.body" => {
            "Nothing could be captured from that area. Try again with the page scrolled to the top."
        }
        "shell.scroll.failed" => "Could Not Finish Scrolling Capture",
        "shell.record.finish_failed.body" => {
            "The recording file could not be finished. Try recording again."
        }
        "shell.record.finish_failed" => "Could Not Finish the Recording",
        "shell.record.at_60" => "Recording at 60 FPS",
        "shell.record.start_failed.body" => {
            "Windows could not start the screen recorder. Try a smaller area or restart Stinkshot."
        }
        "shell.export.failed.body" => "The recording could not be exported.",
        "shell.export.failed.tail" => "The trim window is still open, so you can try Save again.",
        "shell.export.failed" => "Could Not Export the Recording",
        "shell.export.partial.body" => "Part of the export failed.",
        "shell.export.partial.tail" => "What was saved is in History.",
        "shell.saved.mp4_gif" => "MP4 and GIF Saved",
        "shell.saved.gif" => "GIF Saved",
        "shell.saved.recording" => "Recording Saved",
        "shell.pin.default_title" => "Pinned Capture",
        "shell.saved.text" => "Text Copied",
        "shell.saved.updated" => "Capture Updated",
        "shell.saved.pinned" => "Pinned to Screen",
        "shell.saved.copied" => "Copied to Clipboard",
        "shell.saved.history" => "Saved to History",
        "shell.update.off" => "Update Checks Are Off",
        "shell.update.off.body" => {
            "Turn them on in Settings > Privacy. They are the only network feature."
        }
        "shell.update.available.body" => "Open the tray menu to see the release page.",
        "shell.update.current" => "Stinkshot Is Up to Date",
        "shell.update.untrusted" => {
            "The release info failed its signature check, so it was ignored."
        }
        "shell.update.unsigned" => {
            "The latest release has no signed release info yet. Try again later."
        }
        "shell.update.no_answer" => "The update server did not answer. Try again later.",
        "shell.update.failed" => "Could Not Check for Updates",
        "shell.config.save_failed.body" => "The settings file could not be written.",
        "shell.config.save_failed.tail" => "Your changes apply until Stinkshot restarts.",
        "shell.config.save_failed" => "Could Not Save Settings",
        "shell.history_dir.failed.body" => "Stinkshot could not use that folder.",
        "shell.history_dir.failed.tail" => "History stays in the previous folder.",
        "shell.history_dir.failed" => "Could Not Open That History Folder",
        "shell.start.failed" => "Stinkshot could not start. {why}",
        "shell.welcome.key" => {
            "Press {key} to capture a region, or use the tray icon. Start Stinkshot again to open History."
        }
        "shell.hud.gif" => "GIF · {fps} FPS · Up to {w} px Wide · No Audio",
        "shell.hud.mp4" => "MP4 · H.264 · {fps} FPS · {audio}",
        "shell.record.needs_hw" => "{fps} FPS needs hardware encoding.",
        "shell.saved.as" => "Saved {name}",
        "shell.update.available" => "Stinkshot {tag} Is Available",
        "shell.update.current.body" => "Version {v} is the latest release.",
        "shell.tray.update_available" => "Update Available: {tag}",
        "shell.pin.ready" => "{name} is ready to paste.",
        "shell.pin.opacity" => "Opacity {pct}%",
        "shell.error.log_hint" => "Details are in the log (Settings > About > Open Log).",
        "shell.countdown.cancel" => "Esc to Cancel",
        "shell.hud.stop_trim" => "Stop and Trim",
        "shell.hud.resume" => "Resume",
        "shell.hud.pause" => "Pause",
        "shell.hud.microphone" => "Microphone",
        "shell.hud.webcam" => "Webcam",
        "shell.hud.show_input" => "Show Clicks and Keys",
        "shell.hud.stop" => "Stop",
        "shell.hud.problem" => "Recording Problem, Stopping and Keeping What Was Recorded",
        "shell.trim.discard_prompt" => "Discard This Recording?",
        "shell.trim.discard_prompt.body" => "It has not been saved yet. This cannot be undone.",
        "shell.trim.save_prompt" => "Save This Recording?",
        "shell.trim.title" => "Trim Recording",
        "shell.trim.loading" => "Loading Preview",
        "shell.trim.reading" => "Reading Frames",
        "shell.trim.gif_warning" => "Long clips make very large GIFs. MP4 is smaller.",
        "shell.trim.discard" => "Discard",
        "shell.trim.keep_editing" => "Keep Editing",
        "shell.trim.save_untrimmed" => "Save Untrimmed",
        "shell.scroll.cancel" => "Cancel",
        "shell.scroll.stop_here" => "Stop Here",
        "shell.hud.off_in_settings" => "{name} Is Off in Settings > Recording",
        "shell.hud.on" => "{name} On",
        "shell.hud.off" => "{name} Off",
        "shell.hud.stop_trim_key" => "Stop and Trim ({key})",
        "shell.hud.dropped" => "{summary} · {n} dropped",
        "shell.trim.save_prompt.body" => {
            "It has not been saved yet. Save Untrimmed keeps the whole recording as {fmt} in History. Discard asks once more, then deletes it."
        }
        "shell.trim.keep" => "keep {t}",
        "shell.scroll.stitching" => "Stitching Frame {n}",
        "shell.internal.title" => "Something Went Wrong",
        "shell.clipboard.busy_saved" => {
            "Another app is using the clipboard. The capture is saved; press Copy on its toast or in History in a moment."
        }
        "shell.clipboard.refused_saved" => {
            "Windows did not accept the image on the clipboard. The capture is saved in History; try Copy there."
        }
        "shell.clipboard.refused" => {
            "Windows did not accept it on the clipboard. Try again in a moment."
        }
        "shell.item.unreadable" => "The file could not be read. It may have been moved or deleted.",
        "shell.save.history_failed.body" => {
            "The capture could not be written to the history folder."
        }
        "shell.save.history_failed" => "Could Not Save to History",
        "shell.clipboard.failed" => "Could Not Copy to Clipboard",
        "shell.save_as.failed" => "Could Not Save the File",
        "shell.save_as.failed.body" => "The file could not be written there. Try another folder.",
        "shell.save_as.open_failed" => "Could Not Open Save As",
        "shell.save_as.open_failed.body" => {
            "The Save As dialog could not open. The capture is still in History."
        }
        "shell.ocr.nothing" => "Nothing readable in the selection.",
        "shell.ocr.copy_failed" => "Could Not Copy Text",
        "shell.ocr.failed" => "Could Not Recognize Text",
        "shell.ocr.failed.capture" => {
            "Windows could not read text from the capture. Try a larger or sharper selection."
        }
        "shell.ocr.failed.selection" => {
            "Windows could not read text from the selection. Try a larger or sharper selection."
        }
        "shell.item.image_copied" => "Image Copied",
        "shell.item.file_copied" => "File Copied",
        "shell.item.ready" => "Ready to paste.",
        "shell.ocr.item_none" => "This item has no recognizable text.",
        "shell.item.pin_failed" => "Could Not Pin",
        "shell.item.pin_failed.body" => {
            "Only images can be pinned, and this one could not be loaded. It may have been moved or deleted."
        }
        "shell.item.edit_failed" => "Could Not Open for Editing",
        "shell.item.edit_failed.body" => {
            "Only images can be edited, and this one could not be loaded. It may have been moved or deleted."
        }
        "shell.item.in_use" => "Close any program that has it open, then try again.",
        "shell.item.delete_failed" => "Could Not Delete",
        "shell.item.forget_failed.body" => {
            "The file is in the Recycle Bin, but History could not forget it. Restart Stinkshot to refresh History."
        }
        "shell.item.open_failed.body" => {
            "Windows could not open it. Check that the file still exists."
        }
        "shell.item.open_failed" => "Could Not Open",
        "shell.item.not_found" => "Could Not Find That Item",
        "shell.item.not_found.body" => "It may have been moved or deleted.",
        "shell.history.read_failed.body" => {
            "The history index could not be read. Restart Stinkshot and try again."
        }
        "shell.history.read_failed" => "Could Not Read History",
        "shell.url.failed.body" => {
            "Windows could not open your web browser. Check that a default browser is set."
        }
        "shell.url.failed" => "Could Not Open the Page",
        "shell.internal.body" => {
            "Stinkshot hit an internal error and stopped that task. Try again. {hint}"
        }
        "shell.ocr.none" => "No OCR Language Installed",
        "shell.ocr.none.body" => {
            "Add a language with text recognition in Windows Settings > Time & language > Language & region."
        }
        "shell.item.recycle_failed" => {
            "Windows could not move it to the Recycle Bin, so it was kept. {why}"
        }
        "shell.hotkey.row.invalid" => "Not a valid key combination. Record it again.",
        "shell.hotkey.row.duplicate" => "Also set for another action, so only one of them works.",
        "shell.hotkey.row.in_use" => {
            "Already used by another program. Close it or pick a different key."
        }
        "shell.hotkey.row.not_registered" => "Windows could not register it. Pick a different key.",
        "shell.hotkey.unavailable" => {
            "Global hotkeys are unavailable. Use the tray icon, or restart Stinkshot."
        }
        "shell.hotkey.prtsc_failed" => {
            "Windows did not let Stinkshot take over PrtSc, so it may still open the Snipping Tool. Turn off Take Over PrintScreen or try again."
        }
        "shell.hotkey.snip_thread_failed" => {
            "Stinkshot could not start its shortcut listener. Restart Stinkshot and try again."
        }
        "shell.hotkey.snip_failed" => {
            "Windows did not let Stinkshot listen for Win+Shift+S and Win+Shift+R, so they still open the Snipping Tool. Turn the option off and on again, or restart Stinkshot."
        }
        "shell.hotkey.not_set" => "Not Set",
        "shell.toggle.on" => "On",
        "shell.toggle.off" => "Off",
        "shell.about.tagline" => "Local-first screenshots and screen recording for Windows",
        "shell.hotkey.phrase.invalid" => "is not a valid key combination",
        "shell.hotkey.phrase.duplicate" => "is set for both",
        "shell.hotkey.phrase.in_use" => "is already used by another program",
        "shell.hotkey.phrase.not_registered" => "could not be registered with Windows",
        "shell.hotkey.invalid" => {
            "{key} ({action}) {phrase}. Record it again in Settings > Hotkeys."
        }
        "shell.hotkey.duplicate" => {
            "{key} {phrase} {other} and {action}; only {other} works. Change one of them in Settings > Hotkeys."
        }
        "shell.hotkey.in_use" => {
            "{key} ({action}) {phrase}. Close that program or pick another key in Settings > Hotkeys."
        }
        "shell.hotkey.not_registered" => {
            "{key} ({action}) {phrase}. Pick a different key in Settings > Hotkeys."
        }
        "shell.about.version" => "Version {v}",
        "shell.save_as.png" => "PNG image",
        "shell.save_as.jpeg" => "JPEG image",
        "shell.save_as.webp" => "WebP image",
        "shell.save_as.title" => "Save Capture As",
        "shell.trim.save_gif" => "Save GIF",
        "shell.trim.save_mp4" => "Save MP4",
        "shell.tray.help" => "Help",
        "shell.tour.skip" => "Skip Tour",
        "shell.tour.back" => "Back",
        "shell.tour.next" => "Next",
        "shell.tour.done" => "Done",
        "shell.tour.open_help" => "Open Help",
        "shell.tour.hotkeys.title" => "Capture With a Hotkey",
        "shell.tour.hotkeys.body" => "Press {key} to capture a region. Then draw, copy or save.",
        "shell.tour.hotkeys.body_unset" => {
            "Set a region hotkey in Settings > Hotkeys, or use the tray icon."
        }
        "shell.tour.tray.title" => "It Lives in the Tray",
        "shell.tour.tray.body" => "Click the tray icon to capture, double-click for History.",
        "shell.tour.privacy.title" => "Nothing Ever Leaves Your PC",
        "shell.tour.privacy.body" => "Nothing is uploaded or tracked. Update checks are opt-in.",
        "shell.tour.files.title" => "Where Captures Go",
        "shell.tour.files.body" => {
            "Saved here and listed in History. Change it in Settings > History."
        }
        "shell.a11y.item" => "{source}, {w} by {h}, {date}",
        _ => return None,
    })
}

/// Every key above, for the key test.
#[cfg(test)]
pub const KEYS: &[&str] = &[
    "shell.tray.history",
    "shell.tray.settings",
    "shell.tray.quit",
    "shell.action.region",
    "shell.action.window",
    "shell.action.fullscreen",
    "shell.action.scrolling",
    "shell.action.ocr",
    "shell.action.repeat_last",
    "shell.action.pick_color",
    "shell.action.record_mp4",
    "shell.action.record_gif",
    "shell.action.region.desc",
    "shell.action.window.desc",
    "shell.action.fullscreen.desc",
    "shell.action.scrolling.desc",
    "shell.action.ocr.desc",
    "shell.action.repeat_last.desc",
    "shell.action.pick_color.desc",
    "shell.action.record_mp4.desc",
    "shell.action.record_gif.desc",
    "shell.error.disk_full",
    "shell.error.access_denied",
    "shell.error.path_too_long",
    "shell.error.not_found",
    "shell.error.in_use",
    "shell.error.network_path",
    "shell.error.clipboard_busy",
    "shell.error.offline",
    "shell.toast.edit",
    "shell.toast.copy",
    "shell.toast.pin",
    "shell.toast.copy_text",
    "shell.toast.folder",
    "shell.toast.play",
    "shell.toast.edit.tip",
    "shell.toast.copy_file.tip",
    "shell.toast.copy_image.tip",
    "shell.toast.pin.tip",
    "shell.toast.copy_text.tip",
    "shell.toast.folder.tip",
    "shell.toast.dismiss",
    "shell.toast.ocr_missing",
    "shell.tray.update_check_on",
    "shell.tray.offline",
    "shell.pin.copied",
    "shell.pin.copy_failed",
    "shell.pin.copy_image",
    "shell.pin.open_editor",
    "shell.pin.reset_zoom",
    "shell.pin.close",
    "shell.pin.opacity.tip",
    "shell.pin.copy.tip",
    "shell.pin.close.tip",
    "shell.notice.no_last_region",
    "shell.notice.last_region_off_screen",
    "shell.error.window_capture",
    "shell.start.retry",
    "shell.welcome.no_key",
    "shell.hud.mic_system",
    "shell.hud.mic",
    "shell.hud.system_audio",
    "shell.hud.no_audio",
    "shell.hotkeys.inactive",
    "shell.tray.unavailable",
    "shell.tray.unavailable.body",
    "shell.welcome.title",
    "shell.star.failed.body",
    "shell.star.failed",
    "shell.busy.recording",
    "shell.busy.recording.capture",
    "shell.busy.capture_open",
    "shell.busy.capture",
    "shell.busy.recording.edit",
    "shell.capture.no_image.body",
    "shell.capture.screen_failed",
    "shell.capture.no_window",
    "shell.capture.no_window.body",
    "shell.capture.no_monitor.body",
    "shell.ocr.no_text",
    "shell.ocr.empty",
    "shell.selection.lost",
    "shell.selection.lost.body",
    "shell.selection.too_small",
    "shell.selection.too_small.scroll",
    "shell.selection.too_small.record",
    "shell.busy.recording.record",
    "shell.record.temp_failed.body",
    "shell.record.start_failed",
    "shell.record.problem.body",
    "shell.record.problem.tail",
    "shell.record.problem",
    "shell.capture.window_failed",
    "shell.edit.not_yet",
    "shell.scroll.stopped",
    "shell.scroll.stopped.body",
    "shell.scroll.failed.body",
    "shell.scroll.failed",
    "shell.record.finish_failed.body",
    "shell.record.finish_failed",
    "shell.record.at_60",
    "shell.record.start_failed.body",
    "shell.export.failed.body",
    "shell.export.failed.tail",
    "shell.export.failed",
    "shell.export.partial.body",
    "shell.export.partial.tail",
    "shell.saved.mp4_gif",
    "shell.saved.gif",
    "shell.saved.recording",
    "shell.pin.default_title",
    "shell.saved.text",
    "shell.saved.updated",
    "shell.saved.pinned",
    "shell.saved.copied",
    "shell.saved.history",
    "shell.update.off",
    "shell.update.off.body",
    "shell.update.available.body",
    "shell.update.current",
    "shell.update.untrusted",
    "shell.update.unsigned",
    "shell.update.no_answer",
    "shell.update.failed",
    "shell.config.save_failed.body",
    "shell.config.save_failed.tail",
    "shell.config.save_failed",
    "shell.history_dir.failed.body",
    "shell.history_dir.failed.tail",
    "shell.history_dir.failed",
    "shell.start.failed",
    "shell.welcome.key",
    "shell.hud.gif",
    "shell.hud.mp4",
    "shell.record.needs_hw",
    "shell.saved.as",
    "shell.update.available",
    "shell.update.current.body",
    "shell.tray.update_available",
    "shell.pin.ready",
    "shell.pin.opacity",
    "shell.error.log_hint",
    "shell.countdown.cancel",
    "shell.hud.stop_trim",
    "shell.hud.resume",
    "shell.hud.pause",
    "shell.hud.microphone",
    "shell.hud.webcam",
    "shell.hud.show_input",
    "shell.hud.stop",
    "shell.hud.problem",
    "shell.trim.discard_prompt",
    "shell.trim.discard_prompt.body",
    "shell.trim.save_prompt",
    "shell.trim.title",
    "shell.trim.loading",
    "shell.trim.reading",
    "shell.trim.gif_warning",
    "shell.trim.discard",
    "shell.trim.keep_editing",
    "shell.trim.save_untrimmed",
    "shell.scroll.cancel",
    "shell.scroll.stop_here",
    "shell.hud.off_in_settings",
    "shell.hud.on",
    "shell.hud.off",
    "shell.hud.stop_trim_key",
    "shell.hud.dropped",
    "shell.trim.save_prompt.body",
    "shell.trim.keep",
    "shell.scroll.stitching",
    "shell.internal.title",
    "shell.clipboard.busy_saved",
    "shell.clipboard.refused_saved",
    "shell.clipboard.refused",
    "shell.item.unreadable",
    "shell.save.history_failed.body",
    "shell.save.history_failed",
    "shell.clipboard.failed",
    "shell.save_as.failed",
    "shell.save_as.failed.body",
    "shell.save_as.open_failed",
    "shell.save_as.open_failed.body",
    "shell.ocr.nothing",
    "shell.ocr.copy_failed",
    "shell.ocr.failed",
    "shell.ocr.failed.capture",
    "shell.ocr.failed.selection",
    "shell.item.image_copied",
    "shell.item.file_copied",
    "shell.item.ready",
    "shell.ocr.item_none",
    "shell.item.pin_failed",
    "shell.item.pin_failed.body",
    "shell.item.edit_failed",
    "shell.item.edit_failed.body",
    "shell.item.in_use",
    "shell.item.delete_failed",
    "shell.item.forget_failed.body",
    "shell.item.open_failed.body",
    "shell.item.open_failed",
    "shell.item.not_found",
    "shell.item.not_found.body",
    "shell.history.read_failed.body",
    "shell.history.read_failed",
    "shell.url.failed.body",
    "shell.url.failed",
    "shell.internal.body",
    "shell.ocr.none",
    "shell.ocr.none.body",
    "shell.item.recycle_failed",
    "shell.hotkey.row.invalid",
    "shell.hotkey.row.duplicate",
    "shell.hotkey.row.in_use",
    "shell.hotkey.row.not_registered",
    "shell.hotkey.unavailable",
    "shell.hotkey.prtsc_failed",
    "shell.hotkey.snip_thread_failed",
    "shell.hotkey.snip_failed",
    "shell.hotkey.not_set",
    "shell.toggle.on",
    "shell.toggle.off",
    "shell.about.tagline",
    "shell.hotkey.phrase.invalid",
    "shell.hotkey.phrase.duplicate",
    "shell.hotkey.phrase.in_use",
    "shell.hotkey.phrase.not_registered",
    "shell.hotkey.invalid",
    "shell.hotkey.duplicate",
    "shell.hotkey.in_use",
    "shell.hotkey.not_registered",
    "shell.about.version",
    "shell.save_as.png",
    "shell.save_as.jpeg",
    "shell.save_as.webp",
    "shell.save_as.title",
    "shell.trim.save_gif",
    "shell.trim.save_mp4",
    "shell.tray.help",
    "shell.tour.skip",
    "shell.tour.back",
    "shell.tour.next",
    "shell.tour.done",
    "shell.tour.open_help",
    "shell.tour.hotkeys.title",
    "shell.tour.hotkeys.body",
    "shell.tour.hotkeys.body_unset",
    "shell.tour.tray.title",
    "shell.tour.tray.body",
    "shell.tour.privacy.title",
    "shell.tour.privacy.body",
    "shell.tour.files.title",
    "shell.tour.files.body",
    "shell.a11y.item",
];
