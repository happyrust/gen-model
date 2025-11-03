use aios_core::Datetime as SurrealDatetime;
use aios_core::get_db_option;
use rumqttc::{AsyncClient, EventLoop, MqttOptions};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

// 更新e3d文件的消息
#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct SyncE3dFileMsg {
    // 后续还需要加入唯一id，如果开启数据同步？ CDC
    // 这个字段用于存储文件名
    pub file_names: Vec<String>,

    // 还需要加入checkhash, 用于校验文件是否正确
    // 这个字段用于存储文件的hash值，用于校验文件是否正确
    pub file_hashes: Vec<String>,

    // 这个字段用于存储文件服务器的域名或者IP地址
    pub file_server_host: String,

    // bj, sjz, zz
    // 这个字段用于存储文件的地理位置信息
    pub location: String,

    // 加入时间戳, 或者这里开启索引
    // 这个字段用于存储文件的时间戳信息
    pub timestamp: SurrealDatetime,
}

// 对 SyncE3dFileMsg 类型进行 trait 实现
impl SyncE3dFileMsg {
    // 定义一种公共构造函数
    // 这个函数接受两个类型为 String 的向量，并返回 Self 类型，代表 SyncE3dFileMsg
    pub fn new(file_names: Vec<String>, file_hashes: Vec<String>) -> Self {
        Self {
            // 存储参数 file_names 到实例的 file_names 字段
            file_names,
            // 存储参数 file_hashes 到实例的 file_hashes 字段
            file_hashes,
            // 拷贝数据库选项中的 file_server_host 并存储到实例的 file_server_host 字段里
            file_server_host: get_db_option().file_server_host.clone(),
            // 拷贝数据库选项中的 location 并存储到实例的 location 字段里
            location: get_db_option().location.clone(),
            // 使用Default trait 的 default 方法将 timestamp 字段初始化为默认值
            timestamp: Default::default(),
        }
    }
}

// 对从 SyncE3dFileMsg 类型到向量 Vec<u8> 的 Into trait 进行实现
impl Into<Vec<u8>> for SyncE3dFileMsg {
    // 定义转换函数
    fn into(self) -> Vec<u8> {
        // 将 self（代表 SyncE3dFileMsg 实例）序列化为 JSON，
        // 并转换为 Vec<u8> 类型，如遇到错误则直接 unwrap，触发 panic
        serde_json::to_vec(&self).unwrap()
    }
}

// 为`SyncE3dFileMsg`的`Vec<u8>`实现从`Vec<u8>`转换的功能。
impl From<Vec<u8>> for SyncE3dFileMsg {
    // 从`Vec<u8>`创建一个新的`SyncE3dFileMsg`实例的函数。
    fn from(v: Vec<u8>) -> Self {
        // 使用`serde_json::from_slice`函数来从`v`的切片中反序列化`SyncE3dFileMsg`实例。
        // `unwrap`调用则假定反序列化永远不会失败。如果反序列化失败，程序将会panic。
        serde_json::from_slice(v.as_slice()).unwrap()
    }
}

// 定义 MqttInstance 结构体，包含两个公开成员：AsyncClient和EventLoop。
pub struct MqttInstance {
    pub client: AsyncClient,
    pub el: EventLoop,
}

// 创建一个新的Mqtt实例函数。
// 输入参数为str类型的id。
// 返回一个新的MqttInstance。
pub fn new_mqtt_inst(id: &str) -> MqttInstance {
    // 先获取数据库参数
    let db_option = get_db_option();

    // 创建新的MqttOptions，配置 MQTT连接选项。
    // 参数分别是服务器ID，MQTT服务器主机，MQTT服务器端口号。
    let mut mqttoptions = MqttOptions::new(id, db_option.mqtt_host.as_str(), db_option.mqtt_port);

    // 设置清理会话为false
    mqttoptions.set_clean_session(false);

    // 设置保持连接的时间为500秒
    mqttoptions.set_keep_alive(Duration::from_secs(500));
    // 使用默认 TCP 超时设置（无需 TLS/认证，保持简单协议）

    // 使用上述配置和5000毫秒的超时时间创建新的AsyncClient和EventLoop
    let (client, el) = AsyncClient::new(mqttoptions, 5000);

    // 返回新创建的MqttInstance
    MqttInstance { client, el }
}
