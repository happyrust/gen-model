use std::sync::Arc;
use std::time::Duration;
use aios_core::get_db_option;
use rumqttc::{AsyncClient, EventLoop, MqttOptions};
use serde::{Deserialize, Serialize};


//更新e3d文件的消息
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncE3dFileMsg{
    pub file_names: Vec<String>,
    pub file_server_host: String,
    pub location: String, //bj, sjz, zz
}

impl Into<Vec<u8>> for SyncE3dFileMsg {
    fn into(self) -> Vec<u8> {
        serde_json::to_vec(&self).unwrap()
    }
}

impl From<Vec<u8>> for SyncE3dFileMsg {
    fn from(v: Vec<u8>) -> Self {
        serde_json::from_slice(v.as_slice()).unwrap()
    }
}

// #[derive(Clone)]
pub struct MqttInstance {
    pub client: AsyncClient,
    pub el: EventLoop,
}


///每次都单独创建client
// pub fn get_or_create_mqtt_client() -> &'static MqttInstance {
//     static INSTANCE: OnceCell<MqttInstance> = OnceCell::new();
//     INSTANCE.get_or_init(|| {
//         let db_option = get_db_option();
//         let mut mqttoptions = MqttOptions::new(db_option.project_name.as_str(),
//                                                db_option.mqtt_host.as_str(), 1883);
//         mqttoptions.set_keep_alive(Duration::from_secs(5));
//         let (client, el) = AsyncClient::new(mqttoptions, 10);
//         MqttInstance {
//             client,
//             el: Arc::new(el),
//         }
//     })
// }



pub fn new_mqtt_inst(id: &str) -> MqttInstance {
    let db_option = get_db_option();
    let mut mqttoptions = MqttOptions::new(id,
                                           db_option.mqtt_host.as_str(), db_option.mqtt_port);
    mqttoptions.set_clean_session(false);
    mqttoptions.set_keep_alive(Duration::from_secs(50));
    let (client, el) = AsyncClient::new(mqttoptions, 50);
    MqttInstance {
        client,
        el,
    }
}