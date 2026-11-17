//! Input identity and retry policy. All platform calls are made off the window thread.
use super::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputStatus {
    Opening,
    Running,
    Waiting,
    Failed,
}

pub(super) fn devices() -> Result<Vec<InputDevice>, String> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.id().ok());
    let mut result = Vec::new();
    for (index, device) in host.input_devices().map_err(|e| e.to_string())?.enumerate() {
        // One stale or misconfigured endpoint must not hide every other device.
        let (Ok(id), Ok(description)) = (device.id(), device.description()) else {
            continue;
        };
        result.push(InputDevice {
            index,
            name: description.name().to_owned(),
            raw_name: description.name().to_owned(),
            is_default: default.as_ref() == Some(&id),
            id: id.to_string(),
        });
    }
    normalize(&mut result, cfg!(target_os = "linux"));
    Ok(result)
}
fn normalize(devices: &mut Vec<InputDevice>, linux: bool) {
    if !linux {
        return;
    }
    // CPAL 0.17's ALSA default_input_device always returns PCM "default".
    // On the tested Ubuntu setup that alias is broken; route the default choice
    // through the advertised PipeWire plugin, without changing system settings.
    devices.retain(|d| pcm_id(&d.id) != "null" && d.raw_name != "Default Audio Device");
    let preferred = devices
        .iter()
        .position(|d| pcm_id(&d.id) == "pipewire")
        .or_else(|| devices.iter().position(|d| pcm_id(&d.id) == "pulse"))
        .or_else(|| devices.iter().position(|d| d.is_default))
        .or_else(|| (!devices.is_empty()).then_some(0));
    if let Some(index) = preferred {
        for (i, d) in devices.iter_mut().enumerate() {
            d.is_default = i == index;
        }
    }
    let names: Vec<_> = devices.iter().map(|d| d.raw_name.clone()).collect();
    for d in devices {
        if names.iter().filter(|name| *name == &d.raw_name).count() > 1 {
            d.name = format!("{} ({})", d.raw_name, pcm_id(&d.id));
        }
    }
}
fn pcm_id(id: &str) -> &str {
    id.split_once(':').map_or(id, |(_, pcm)| pcm)
}

pub(super) struct Target {
    index: Option<usize>,
    id: Option<String>,
    name: Option<String>,
}
impl Target {
    pub fn new(index: Option<usize>, id: Option<&str>, name: Option<&str>) -> Self {
        Self {
            index,
            id: id.map(str::to_owned),
            name: name.map(str::to_owned),
        }
    }
    pub fn choose(&mut self) -> Result<(cpal::Device, InputDevice), String> {
        let devices = devices()?;
        let info = select(
            &devices,
            self.id.as_deref(),
            self.name.as_deref(),
            self.index,
        )
        .ok_or_else(|| "Audio input device is unavailable".to_string())?
        .clone();
        // Pin even the default/index choice to the actual device, so an unplug
        // never silently switches to another microphone or a shifted index.
        self.id = Some(info.id.clone());
        self.name = Some(info.raw_name.clone());
        self.index = None;
        let id = info
            .id
            .parse::<cpal::DeviceId>()
            .map_err(|e| e.to_string())?;
        cpal::default_host()
            .device_by_id(&id)
            .map(|d| (d, info))
            .ok_or_else(|| "Audio input device is unavailable".to_string())
    }
}
fn select<'a>(
    devices: &'a [InputDevice],
    id: Option<&str>,
    name: Option<&str>,
    index: Option<usize>,
) -> Option<&'a InputDevice> {
    if let Some(id) = id {
        devices
            .iter()
            .find(|d| d.id == id)
            .or_else(|| name.and_then(|name| devices.iter().find(|d| d.raw_name == name)))
    } else if let Some(index) = index {
        devices.iter().find(|d| d.index == index)
    } else {
        devices.iter().find(|d| d.is_default)
    }
}

pub(super) struct Retry {
    started: Instant,
    connected: bool,
}
impl Retry {
    pub fn new(started: Instant) -> Self {
        Self {
            started,
            connected: false,
        }
    }
    pub fn connected(&mut self) {
        self.connected = true;
    }
    pub fn was_connected(&self) -> bool {
        self.connected
    }
    pub fn retry(&self, now: Instant) -> bool {
        self.connected || now.duration_since(self.started) < Duration::from_secs(10)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn device(index: usize, id: &str, name: &str) -> InputDevice {
        InputDevice {
            index,
            id: id.into(),
            name: name.into(),
            raw_name: name.into(),
            is_default: false,
        }
    }
    #[test]
    fn linux_labels_keep_raw_indexes_and_hide_broken_aliases() {
        let mut list = vec![
            device(0, "alsa:null", "Null"),
            device(1, "alsa:default", "Default Audio Device"),
            device(2, "alsa:pipewire", "PipeWire"),
            device(4, "alsa:hw:CARD=1,DEV=0", "SSL"),
            device(8, "alsa:plughw:CARD=1,DEV=0", "SSL"),
        ];
        normalize(&mut list, true);
        assert_eq!(list.iter().map(|d| d.index).collect::<Vec<_>>(), [2, 4, 8]);
        assert!(list[0].is_default);
        assert_eq!(list[1].name, "SSL (hw:CARD=1,DEV=0)");
        assert_eq!(list[2].raw_name, "SSL");
    }
    #[test]
    fn identity_precedes_name_and_never_reuses_an_old_index() {
        let list = vec![device(0, "new", "SSL"), device(4, "saved", "Other")];
        assert_eq!(
            select(&list, Some("saved"), Some("SSL"), Some(0))
                .unwrap()
                .index,
            4
        );
        assert_eq!(
            select(&list, Some("gone"), Some("SSL"), Some(4))
                .unwrap()
                .index,
            0
        );
        assert!(select(&list, Some("gone"), Some("Missing"), Some(4)).is_none());
    }
    #[test]
    fn initial_open_is_bounded_but_reconnection_waits_indefinitely() {
        let now = Instant::now();
        let mut retry = Retry::new(now);
        assert!(retry.retry(now + Duration::from_secs(9)));
        assert!(!retry.retry(now + Duration::from_secs(10)));
        retry.connected();
        assert!(retry.retry(now + Duration::from_secs(600)));
    }
}
