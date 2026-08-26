use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransportTechnology {
    pub name: String,
    pub family: TransportTechnologyFamily,
    pub layer: TransportTechnologyLayer,
    pub built_on: Vec<String>,
    pub reusable_by: Vec<String>,
    pub events: Vec<TransportEventKind>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportTechnologyFamily {
    FileSystem,
    IpNetwork,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportTechnologyLayer {
    Storage,
    Watch,
    Network,
    Transport,
    Session,
    Application,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum TransportEventKind {
    FileCreated,
    FileChanged,
    FileClosed,
    FileReady,
    FileMoved,
    FileDeleted,
    PacketReceived,
    ConnectionAccepted,
    RequestReceived,
    ResponseReceived,
    TimerElapsed,
}

pub fn core_transport_tree() -> Vec<TransportTechnology> {
    let mut tree = file_transport_tree();
    tree.extend(ip_transport_tree());
    tree
}

pub fn file_transport_tree() -> Vec<TransportTechnology> {
    vec![
        TransportTechnology {
            name: "file-system".to_string(),
            family: TransportTechnologyFamily::FileSystem,
            layer: TransportTechnologyLayer::Storage,
            built_on: Vec::new(),
            reusable_by: vec!["file-watch".to_string(), "file-poll".to_string()],
            events: Vec::new(),
        },
        TransportTechnology {
            name: "file-watch".to_string(),
            family: TransportTechnologyFamily::FileSystem,
            layer: TransportTechnologyLayer::Watch,
            built_on: vec!["file-system".to_string()],
            reusable_by: vec!["file".to_string()],
            events: vec![
                TransportEventKind::FileCreated,
                TransportEventKind::FileChanged,
                TransportEventKind::FileClosed,
                TransportEventKind::FileMoved,
                TransportEventKind::FileDeleted,
            ],
        },
        TransportTechnology {
            name: "file-poll".to_string(),
            family: TransportTechnologyFamily::FileSystem,
            layer: TransportTechnologyLayer::Watch,
            built_on: vec!["file-system".to_string()],
            reusable_by: vec!["file".to_string()],
            events: vec![
                TransportEventKind::TimerElapsed,
                TransportEventKind::FileReady,
            ],
        },
        TransportTechnology {
            name: "file".to_string(),
            family: TransportTechnologyFamily::FileSystem,
            layer: TransportTechnologyLayer::Application,
            built_on: vec!["file-watch".to_string(), "file-poll".to_string()],
            reusable_by: Vec::new(),
            events: vec![TransportEventKind::FileReady],
        },
    ]
}

pub fn ip_transport_tree() -> Vec<TransportTechnology> {
    vec![
        TransportTechnology {
            name: "ip".to_string(),
            family: TransportTechnologyFamily::IpNetwork,
            layer: TransportTechnologyLayer::Network,
            built_on: Vec::new(),
            reusable_by: vec!["tcp".to_string(), "udp".to_string()],
            events: vec![TransportEventKind::PacketReceived],
        },
        TransportTechnology {
            name: "tcp".to_string(),
            family: TransportTechnologyFamily::IpNetwork,
            layer: TransportTechnologyLayer::Transport,
            built_on: vec!["ip".to_string()],
            reusable_by: vec![
                "http".to_string(),
                "grpc".to_string(),
                "ftp".to_string(),
                "sftp".to_string(),
                "mllp".to_string(),
                "mqtt".to_string(),
            ],
            events: vec![TransportEventKind::ConnectionAccepted],
        },
        TransportTechnology {
            name: "udp".to_string(),
            family: TransportTechnologyFamily::IpNetwork,
            layer: TransportTechnologyLayer::Transport,
            built_on: vec!["ip".to_string()],
            reusable_by: vec!["dns".to_string(), "syslog".to_string()],
            events: vec![TransportEventKind::PacketReceived],
        },
        TransportTechnology {
            name: "http".to_string(),
            family: TransportTechnologyFamily::IpNetwork,
            layer: TransportTechnologyLayer::Application,
            built_on: vec!["tcp".to_string()],
            reusable_by: vec![
                "soap".to_string(),
                "rest".to_string(),
                "webhook".to_string(),
            ],
            events: vec![
                TransportEventKind::RequestReceived,
                TransportEventKind::ResponseReceived,
            ],
        },
        TransportTechnology {
            name: "grpc".to_string(),
            family: TransportTechnologyFamily::IpNetwork,
            layer: TransportTechnologyLayer::Application,
            built_on: vec!["http".to_string()],
            reusable_by: Vec::new(),
            events: vec![
                TransportEventKind::RequestReceived,
                TransportEventKind::ResponseReceived,
            ],
        },
    ]
}

pub fn depends_on(technology: &str, dependency: &str) -> bool {
    core_transport_tree()
        .iter()
        .find(|item| item.name == technology)
        .map(|item| item.built_on.iter().any(|base| base == dependency))
        .unwrap_or(false)
}

pub fn family_of(technology: &str) -> Option<TransportTechnologyFamily> {
    core_transport_tree()
        .into_iter()
        .find(|item| item.name == technology)
        .map(|item| item.family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tcp_and_udp_are_built_on_ip() {
        assert!(depends_on("tcp", "ip"));
        assert!(depends_on("udp", "ip"));
    }

    #[test]
    fn grpc_is_built_on_http() {
        assert!(depends_on("grpc", "http"));
    }

    #[test]
    fn hierarchy_stops_at_ip_for_now() {
        let ip = core_transport_tree()
            .into_iter()
            .find(|item| item.name == "ip")
            .expect("ip technology expected");

        assert!(ip.built_on.is_empty());
    }

    #[test]
    fn file_is_parallel_to_ip_networking() {
        assert_eq!(
            family_of("file"),
            Some(TransportTechnologyFamily::FileSystem)
        );
        assert_eq!(family_of("ip"), Some(TransportTechnologyFamily::IpNetwork));
        assert!(!depends_on("file", "ip"));
    }

    #[test]
    fn file_tree_has_file_events() {
        let file_watch = core_transport_tree()
            .into_iter()
            .find(|item| item.name == "file-watch")
            .expect("file-watch technology expected");

        assert!(file_watch.events.contains(&TransportEventKind::FileCreated));
        assert!(file_watch.events.contains(&TransportEventKind::FileClosed));
    }
}
