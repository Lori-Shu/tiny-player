// use image::EncodableLayout;
// use reqwest::{
//     Body, Client,
//     header::{HeaderMap, HeaderValue},
// };
// use serde::{Deserialize, Serialize};
// use tokio::sync::mpsc::{Receiver, Sender};

// use crate::PlayerResult;
// const TRANSLATE_KEY: &str = include_str!("./resources/translate_key");
// const APP_ID: &str = "20260311002570648";
// const BAIDU_TRANSLATE_URL: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";
// const SALT: &str = "江畔何人初见月";
// pub struct Translater {
//     client: Client,
//     str_receiver: Receiver<String>,
//     str_sender: Sender<String>,
// }
// impl Translater {
//     pub fn new(str_receiver: Receiver<String>, str_sender: Sender<String>) -> PlayerResult<Self> {
//         let mut header = HeaderMap::new();
//         header
//             .insert(
//                 "content-type",
//                 HeaderValue::from_str("application/x-www-form-urlencoded")?,
//             )
//             .ok_or(anyhow::Error::msg("insert reqwest header failed!"))?;
//         let client = Client::builder().default_headers(header).build()?;
//         Ok(Self {
//             client,
//             str_receiver,
//             str_sender,
//         })
//     }
//     pub async fn send_translate_request(&self) -> PlayerResult<()> {
//         let mut sign_str = APP_ID.to_string();
//         sign_str.extend("江月何年初照人");
//         sign_str.extend(SALT);
//         sign_str.extend(TRANSLATE_KEY);
//         let sign = String::from_utf8(md5::Digest::digest(sign_str).to_vec())?;

//         let params = [
//             ("q", "江月何年初照人"),
//             ("from", "zh"),
//             ("to", "en"),
//             ("appid", APP_ID),
//             ("salt", SALT),
//             ("sign", &sign),
//         ];
//         let response = self
//             .client
//             .post(BAIDU_TRANSLATE_URL)
//             .form(form)
//             .send()
//             .await?;
//         if response.status().is_success() {
//             let translate_res = serde_json::from_slice::<TranslateResponse>(response.bytes()?)?;
//             info!("translate response{}",translate_res.to_json());
//         }
//     }
// }
// #[derive(Debug, Serialize,Deserialize)]
// struct TranslateResult {
//     src: String,
//     dst: String,
// }
// #[derive(Debug, Serialize,Deserialize)]
// struct TranslateResponse {
//     from: String,
//     to: String,
//     trans_result: Vec<TranslateResult>,
// }
// #[cfg(test)]
// mod test{
//     #[test]
//      async fn test_translate(){
//          let channel_0 = mpsc::channel(1);
//          let channel_1 = mpsc::channel(1);
//          let translater = Translater::new(channel_0.1, channel_1.0).unwrap();
//          translater.send_translate_request().await.unwrap();
//     }

// }
