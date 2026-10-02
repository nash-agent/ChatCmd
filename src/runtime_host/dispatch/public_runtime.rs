use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct PublicExecutionRuntime {
    pub(super) device_id: String,
    pub(super) name: String,
    pub(super) platform: String,
    pub(super) architecture: String,
    pub(super) app_version: String,
    pub(super) online: bool,
}

pub(super) fn public_device(device: chatcmd_runtime::DeviceDescriptor) -> PublicExecutionRuntime {
    PublicExecutionRuntime {
        device_id: "default-runtime".to_owned(),
        name: "workspace-runtime".to_owned(),
        platform: device.platform,
        architecture: device.architecture,
        app_version: device.app_version,
        online: device.online,
    }
}

#[cfg(test)]
mod tests {
    use super::public_device;

    #[test]
    fn public_device_hides_host_identity_and_omits_redundant_fields() {
        let public = public_device(chatcmd_runtime::DeviceDescriptor {
            device_id: "real-device-id".to_owned(),
            machine_id: Some("real-machine-id".to_owned()),
            name: "DESKTOP-SECRET".to_owned(),
            platform: "windows".to_owned(),
            os_version: "Windows 11 Home".to_owned(),
            architecture: "x86_64".to_owned(),
            app_version: "0.1.0".to_owned(),
            online: true,
        });

        assert_eq!(public.device_id, "default-runtime");
        assert_eq!(public.name, "workspace-runtime");
        assert_eq!(public.platform, "windows");
        assert_eq!(public.architecture, "x86_64");
        assert!(public.online);

        let serialized = serde_json::to_value(&public).expect("serialize public runtime");
        assert_eq!(serialized["deviceId"], "default-runtime");
        assert_eq!(serialized["name"], "workspace-runtime");
        assert_eq!(serialized["platform"], "windows");
        assert_eq!(serialized["architecture"], "x86_64");
        assert_eq!(serialized["appVersion"], "0.1.0");
        assert_eq!(serialized["online"], true);
        assert!(serialized.get("machineId").is_none());
        assert!(serialized.get("osVersion").is_none());
    }
}
