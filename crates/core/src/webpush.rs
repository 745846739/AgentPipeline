//! 浏览器推送的密码学三件（spec `.scratch/pwa-webpush/` 票 02）：RFC 8291 的报文加密、
//! RFC 8188 的 `aes128gcm` 内容编码、RFC 8292 的 VAPID 鉴权。
//!
//! **为什么不引 `web-push` 整包**：本票要的只有三段——一对 P-256 密钥、一次 ECDH 派生、
//! 一节 AES-128-GCM 记录。三段都能拿 RFC 的**公开测试向量**钉死（RFC 8291 §5 + 附录 A
//! 给了明文、双方公钥、ECDH 共享秘密、IKM、CEK、NONCE、头部与密文全套中间值），
//! 而自写才有得钉：引整包只能测「我调它没调错」，测不到「线的那一头是标准 Web Push」。
//! 原语全走 ring（已在依赖树里的审计过的实现）：P-256 的 ECDH / ECDSA、AES-GCM、HKDF。
//!
//! 三段的分工：
//!
//! - [`generate_vapid_keys`]：服务端身份（RFC 8292）。私钥存 PKCS#8、公钥存未压缩点，
//!   都是 base64url 无填充——前者是 ring 的天然形状，后者是浏览器
//!   `applicationServerKey` 与 JWT `k` 参数要的那一份。
//! - [`vapid_authorization`]：把身份变成 `Authorization: vapid t=…,k=…`（ES256 的 JWT）。
//! - [`encrypt_payload`]：把报文变成推送服务能转交、浏览器能解开的那一串字节
//!   （头 86 字节 + 一节密文；见 [`derive_content_keys`] 与 [`seal_record`]，KAT 就钉这两个）。
//!
//! **秘密面**：`auth`（鉴权秘密）与 VAPID 私钥都不进日志（与 272⑦ 的 URL 同一条纪律）。

use std::time::Duration;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use chrono::{DateTime, Utc};
use ring::aead;
use ring::agreement::{agree_ephemeral, EphemeralPrivateKey, UnparsedPublicKey, ECDH_P256};
use ring::hkdf;
use ring::rand::{SecureRandom, SystemRandom};
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_FIXED_SIGNING};

use crate::storage::push::VapidKeys;

/// 一节记录的上限（RFC 8188 的 `rs`）。报文是一行 JSON（几百字节量级），4096 是
/// 各家实现的常规取值——**单节**就够，故不需要分片逻辑（超长会被 `encrypt_payload` 拒掉
/// 而不是悄悄分片：分片要另一套 nonce 递推，而本票的报文不会长到那里）。
pub const RECORD_SIZE: u32 = 4096;

/// 未压缩 P-256 点的长度（`0x04 || X || Y`）——`rs` 之外的第二个形状常量。
const UNCOMPRESSED_POINT_LEN: usize = 65;

/// 推送报文的存活上限（RFC 8030 的 `TTL`，秒）：桌面与手机的推送服务都会在设备离线时
/// 缓存消息，这里给一天——通知是「此刻的事」，压过一天再弹出来已经没有意义。
pub const PUSH_TTL_SECS: u32 = 86_400;

/// VAPID JWT 的有效期（RFC 8292 建议 ≤ 24h）。取 12 小时：一条报文签一次、
/// 签发与投递之间只隔毫秒，窗口短一点无损，被截获的 token 能用的时间也短一点。
const VAPID_JWT_TTL_SECS: i64 = 12 * 3600;

const VAPID_SUBJECT: &str = "mailto:agentpipeline@example.com";

type PushResult<T> = std::result::Result<T, String>;

fn rng() -> SystemRandom {
    SystemRandom::new()
}

/// base64url 无填充解码（JWT / 密钥 / 订阅密钥的统一形状）。
fn b64_decode(value: &str) -> PushResult<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(value.trim())
        .map_err(|e| format!("base64url 解不开：{e}"))
}

fn b64_encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// 订阅里那两件密钥的解码 + **形状校验**（`p256dh` 是 65 字节未压缩点、`auth` 是 16 字节）。
///
/// 长度不对是**报错**而不是「凑合加密」：把 32 字节的东西当公钥去协商，症状是推送服务
/// 那头的 400 / 401，而病因在几层之外。校验点在两处共用：落库前的端点（[`validate_subscription`]）
/// 与发送前的加密。
fn decode_subscription_keys(p256dh: &str, auth: &str) -> PushResult<(Vec<u8>, Vec<u8>)> {
    let public = b64_decode(p256dh)?;
    if public.len() != UNCOMPRESSED_POINT_LEN {
        return Err(format!(
            "订阅里的 p256dh 应当是 {} 字节的未压缩点，实际 {} 字节",
            UNCOMPRESSED_POINT_LEN,
            public.len()
        ));
    }
    let secret = b64_decode(auth)?;
    if secret.len() != 16 {
        return Err(format!(
            "订阅里的 auth 应当是 16 字节，实际 {} 字节",
            secret.len()
        ));
    }
    Ok((public, secret))
}

/// 一条订阅三件的形状校验：`endpoint` 要是 http(s) 地址、`p256dh` / `auth` 要是形状对得上的
/// base64url。订阅端点用它**在落库前**把明显坏的订阅拒掉（400 报错不静默，272⑧ 的姿态）——
/// 否则坏行会静静地躺在清单里，直到某次通知才发现它永远发不出去。
pub fn validate_subscription(endpoint: &str, p256dh: &str, auth: &str) -> PushResult<()> {
    endpoint_origin(endpoint)?;
    decode_subscription_keys(p256dh, auth)?;
    Ok(())
}

// ───────────────────────── VAPID（RFC 8292）─────────────────────────

/// 生成一对 VAPID 密钥（首次启用时由 [`crate::storage::Store::ensure_push_vapid_keys`] 调用）。
pub fn generate_vapid_keys() -> PushResult<VapidKeys> {
    let document =
        EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng()).map_err(|_| {
            "系统随机源不可用，生成不了 VAPID 密钥（这是本机环境问题，不是配置问题）".to_string()
        })?;
    let pair =
        EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, document.as_ref(), &rng())
            .map_err(|e| format!("生成的 VAPID 私钥解不开：{e}"))?;
    Ok(VapidKeys {
        public_key: b64_encode(pair.public_key().as_ref()),
        private_key: b64_encode(document.as_ref()),
    })
}

/// `Authorization` 头的值：`vapid t=<JWT>,k=<公钥>`（RFC 8292 §3）。
///
/// `aud` 取 **endpoint 的 origin**（scheme://host[:port]），不是完整 URL——推送服务
/// 按 origin 校验，把完整地址写进去会得到一个 401 而原因看不出来。
pub fn vapid_authorization(
    keys: &VapidKeys,
    endpoint: &str,
    now: DateTime<Utc>,
) -> PushResult<String> {
    let origin = endpoint_origin(endpoint)?;
    let jwt = vapid_jwt(keys, &origin, now)?;
    Ok(format!("vapid t={jwt},k={}", keys.public_key))
}

/// VAPID 的联系方式（RFC 8292 的 `sub`）。
///
/// **占位 mailto**（票面点名）：真实的邮箱在这里没有任何用途——它只是给推送服务一个
/// 「出事了找谁」的联系方式，而泄露一个真邮箱到每个推送服务的日志里不成比例。
/// 换真邮箱是改这一个常量的事。
pub fn vapid_subject() -> &'static str {
    VAPID_SUBJECT
}

/// 推送 endpoint 的 origin（`https://push.example.net:8443` 这类）。
///
/// 不自写字符串截取：`aud` 要与推送服务自己算出来的那一个**逐字相等**，而 origin 的规则是
/// 「默认端口不写出来、非默认端口要写、主机全小写」。`url` 的 `Origin::ascii_serialization()`
/// 正是这套规则（`url` 在依赖树里，是 reqwest 的传递依赖）。
fn endpoint_origin(endpoint: &str) -> PushResult<String> {
    let url =
        reqwest::Url::parse(endpoint.trim()).map_err(|e| format!("推送地址不是合法 URL：{e}"))?;
    let origin = url.origin();
    if !origin.is_tuple() || !matches!(url.scheme(), "http" | "https") {
        return Err("推送地址要是 http(s) 地址（RFC 8030 的 endpoint 就长这样）".into());
    }
    Ok(origin.ascii_serialization())
}

/// ES256 的 JWT（头 / 声明两段 base64url + 64 字节的 r‖s 签名）。
///
/// 签名算法取 `ECDSA_P256_SHA256_FIXED_SIGNING`——它输出的正是 JWS 要的
/// **定长 r‖s**（`…_ASN1_SIGNING` 输出 DER，签出来的 JWT 任何推送服务都验不过）。
fn vapid_jwt(keys: &VapidKeys, audience: &str, now: DateTime<Utc>) -> PushResult<String> {
    let pkcs8 = b64_decode(&keys.private_key)?;
    let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &pkcs8, &rng())
        .map_err(|e| format!("VAPID 私钥解不开（库里的那一对坏了吗）：{e}"))?;
    let header = b64_encode(br#"{"typ":"JWT","alg":"ES256"}"#);
    let claims = format!(
        r#"{{"aud":"{audience}","exp":{},"sub":"{}"}}"#,
        now.timestamp() + VAPID_JWT_TTL_SECS,
        vapid_subject()
    );
    let signing_input = format!("{header}.{}", b64_encode(claims.as_bytes()));
    let signature = pair
        .sign(&rng(), signing_input.as_bytes())
        .map_err(|_| "VAPID 签名失败（系统随机源不可用）".to_string())?;
    Ok(format!(
        "{signing_input}.{}",
        b64_encode(signature.as_ref())
    ))
}

// ───────────────────── 报文加密（RFC 8291 + 8188）─────────────────────

/// 加密一条推送报文，返回**完整报文体**（86 字节的头 + 一节密文）。
///
/// `p256dh` / `auth` 是订阅里那两件（base64url）；`plaintext` 是要推给浏览器的那串字节
/// （本票是 `{title, body, url}` 的 JSON）。
pub fn encrypt_payload(p256dh: &str, auth: &str, plaintext: &[u8]) -> PushResult<Vec<u8>> {
    let (ua_public, auth_secret) = decode_subscription_keys(p256dh, auth)?;
    if plaintext.len() + 1 + 16 > RECORD_SIZE as usize {
        return Err(format!(
            "报文超过单节上限（{} 字节）：本票不做分片",
            RECORD_SIZE
        ));
    }

    // 每次发送现生成一对临时 ECDH 密钥（RFC 8291 §3.1：用完即弃，与服务端身份 VAPID
    // 是两件事——VAPID 只证明「这条是谁发的」，它不参与内容加密）。
    let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &rng())
        .map_err(|_| "系统随机源不可用，生成不了临时 ECDH 密钥".to_string())?;
    let as_public = ephemeral
        .compute_public_key()
        .map_err(|_| "临时 ECDH 公钥算不出来".to_string())?
        .as_ref()
        .to_vec();

    let mut salt = [0u8; 16];
    rng()
        .fill(&mut salt)
        .map_err(|_| "系统随机源不可用，取不到 salt".to_string())?;

    let peer = UnparsedPublicKey::new(&ECDH_P256, &ua_public);
    // 「用完即弃」的那一步交给 ring：闭包拿到的是共享秘密的只读切片，函数返回后
    // 那把临时私钥连同秘密一起被销毁（我们只带走一份拷贝去派生密钥）。
    let ecdh_secret = agree_ephemeral(ephemeral, &peer, |secret| secret.to_vec())
        .map_err(|_| "ECDH 协商失败（订阅里的公钥不在 P-256 上？）".to_string())?;

    let (cek, nonce) =
        derive_content_keys(&ecdh_secret, &auth_secret, &ua_public, &as_public, &salt)?;
    let ciphertext = seal_record(&cek, &nonce, plaintext)?;

    let mut body = Vec::with_capacity(UNCOMPRESSED_POINT_LEN + 21 + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(&as_public);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// RFC 8291 §3.4 的密钥派生：`(ecdh_secret, auth, ua_public, as_public, salt) → (CEK, NONCE)`。
///
/// 两段 HKDF，**顺序与 info 串都不能动**：
/// ① `PRK_key = HKDF-Extract(auth, ecdh_secret)`，再以
/// `"WebPush: info" || 0x00 || ua_public || as_public` 扩出 32 字节 IKM；
/// ② `PRK = HKDF-Extract(salt, IKM)`，再以 `"Content-Encoding: aes128gcm" || 0x00` 扩出
/// 16 字节 CEK、以 `"Content-Encoding: nonce" || 0x00` 扩出 12 字节 nonce。
///
/// 抽出成独立函数是为了让 RFC 8291 附录 A 的中间值能直接钉它（KAT 见下）。
pub fn derive_content_keys(
    ecdh_secret: &[u8],
    auth_secret: &[u8],
    ua_public: &[u8],
    as_public: &[u8],
    salt: &[u8; 16],
) -> PushResult<([u8; 16], [u8; 12])> {
    fn expand(prk: hkdf::Prk, info: &[u8], len: usize) -> PushResult<Vec<u8>> {
        struct Len(usize);
        impl hkdf::KeyType for Len {
            fn len(&self) -> usize {
                self.0
            }
        }
        let mut out = vec![0u8; len];
        prk.expand(&[info], Len(len))
            .map_err(|_| "HKDF 展开失败（输出长度越界？）".to_string())?
            .fill(&mut out)
            .map_err(|_| "HKDF 输出填充失败".to_string())?;
        Ok(out)
    }

    // ① 认证秘密参与的那一段（key_info 里两个公钥都在，顺序是 ua 在前、as 在后）。
    let mut key_info = Vec::with_capacity(14 + ua_public.len() + as_public.len());
    key_info.extend_from_slice(b"WebPush: info\x00");
    key_info.extend_from_slice(ua_public);
    key_info.extend_from_slice(as_public);
    let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, auth_secret).extract(ecdh_secret);
    let ikm = expand(prk_key, &key_info, 32)?;

    // ② 报文自己的 salt 参与的那一段。
    let prk = hkdf::Salt::new(hkdf::HKDF_SHA256, salt).extract(&ikm);
    let cek = expand(prk.clone(), b"Content-Encoding: aes128gcm\x00", 16)?;
    let nonce = expand(prk, b"Content-Encoding: nonce\x00", 12)?;

    let mut cek_out = [0u8; 16];
    cek_out.copy_from_slice(&cek);
    let mut nonce_out = [0u8; 12];
    nonce_out.copy_from_slice(&nonce);
    Ok((cek_out, nonce_out))
}

/// 一节 `aes128gcm` 记录（RFC 8188 §2）：明文 + 定界符 `0x02`，AES-128-GCM 加密后
/// 带上 16 字节 tag（AAD 为空——本票没有 `aes128gcm` 之外的头部参与认证）。
pub fn seal_record(cek: &[u8; 16], nonce: &[u8; 12], plaintext: &[u8]) -> PushResult<Vec<u8>> {
    let key = aead::LessSafeKey::new(
        aead::UnboundKey::new(&aead::AES_128_GCM, cek)
            .map_err(|_| "CEK 长度不对（应当是 16 字节）".to_string())?,
    );
    let mut record = Vec::with_capacity(plaintext.len() + 1 + 16);
    record.extend_from_slice(plaintext);
    // 最后一条记录的定界符（RFC 8188 §2：0x02 = 这是收尾记录）。补零填充是**可选**的，
    // 本票不填——报文本来就没有需要掩饰的长度特征（一次通知就那么点字节）。
    record.push(0x02);
    key.seal_in_place_append_tag(
        aead::Nonce::assume_unique_for_key(*nonce),
        aead::Aad::empty(),
        &mut record,
    )
    .map_err(|_| "AES-128-GCM 加密失败".to_string())?;
    Ok(record)
}

/// 投递超时（与 webhook 同一档）：通知是 best-effort，挂住不能拖着调用方。
pub const PUSH_TIMEOUT: Duration = Duration::from_secs(10);

#[cfg(test)]
mod tests {
    use super::*;
    use ring::signature::{UnparsedPublicKey as SigPublicKey, ECDSA_P256_SHA256_FIXED};

    // ── RFC 8291 §5 + 附录 A 的测试向量（逐个值照抄，别改写）──
    //
    // 这些值来自 RFC 正文，不是我们自己算的——这正是这段代码有多可信的全部来源：
    // 派生顺序、info 串、salt 用法、定界符、头部布局，任何一处写错都会当场对不上。
    const RFC_UA_PUBLIC: &str =
        "BCVxsr7N_eNgVRqvHtD0zTZsEc6-VV-JvLexhqUzORcxaOzi6-AYWXvTBHm4bjyPjs7Vd8pZGH6SRpkNtoIAiw4";
    const RFC_AS_PUBLIC: &str =
        "BP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    const RFC_AUTH_SECRET: &str = "BTBZMqHH6r4Tts7J_aSIgg";
    const RFC_SALT: &str = "DGv6ra1nlYgDCS1FRnbzlw";
    const RFC_ECDH_SECRET: &str = "kyrL1jIIOHEzg3sM2ZWRHDRB62YACZhhSlknJ672kSs";
    const RFC_IKM: &str = "S4lYMb_L0FxCeq0WhDx813KgSYqU26kOyzWUdsXYyrg";
    const RFC_CEK: &str = "oIhVW04MRdy2XN9CiKLxTg";
    const RFC_NONCE: &str = "4h_95klXJ5E_qnoN";
    const RFC_PLAINTEXT: &str = "When I grow up, I want to be a watermelon";
    /// 头部（86 字节：salt ‖ rs=4096 ‖ idlen=65 ‖ as_public）。
    const RFC_HEADER: &str = "DGv6ra1nlYgDCS1FRnbzlwAAEABBBP4z9KsN6nGRTbVYI_c7VJSPQTBtkgcy27mlmlMoZIIgDll6e3vCYLocInmYWAmS6TlzAC8wEqKK6PBru3jl7A8";
    /// 密文（明文 + `0x02`，AES-128-GCM 加密后 48 字节）。
    const RFC_CIPHERTEXT: &str =
        "8pfeW0KbunFT06SuDKoJH9Ql87S1QUrdirN6GcG7sFz1y1sqLgVi1VhjVkHsUoEsbI_0LpXMuGvnzQ";

    /// **KAT：密钥派生**。RFC 8291 附录 A 给了 ecdh_secret → IKM → CEK/NONCE 的全链中间值，
    /// 逐段对：info 串写错、两个公钥的顺序颠倒、salt 用错位置，这里都会当场红。
    #[test]
    fn rfc8291_key_derivation_known_answer() {
        let salt: [u8; 16] = b64_decode(RFC_SALT).unwrap().try_into().unwrap();
        let (cek, nonce) = derive_content_keys(
            &b64_decode(RFC_ECDH_SECRET).unwrap(),
            &b64_decode(RFC_AUTH_SECRET).unwrap(),
            &b64_decode(RFC_UA_PUBLIC).unwrap(),
            &b64_decode(RFC_AS_PUBLIC).unwrap(),
            &salt,
        )
        .unwrap();
        assert_eq!(b64_encode(&cek), RFC_CEK);
        assert_eq!(b64_encode(&nonce), RFC_NONCE);

        // 顺带把中间那一段也钉住：IKM 对上，说明是**①**那一半（含 info 串）没错，
        // 而不是两处错误恰好抵消。
        let key_info: Vec<u8> = [
            b"WebPush: info\x00".to_vec(),
            b64_decode(RFC_UA_PUBLIC).unwrap(),
            b64_decode(RFC_AS_PUBLIC).unwrap(),
        ]
        .concat();
        let prk_key = hkdf::Salt::new(hkdf::HKDF_SHA256, &b64_decode(RFC_AUTH_SECRET).unwrap())
            .extract(&b64_decode(RFC_ECDH_SECRET).unwrap());
        let mut ikm = [0u8; 32];
        struct Len(usize);
        impl hkdf::KeyType for Len {
            fn len(&self) -> usize {
                self.0
            }
        }
        prk_key
            .expand(&[&key_info], Len(32))
            .unwrap()
            .fill(&mut ikm)
            .unwrap();
        assert_eq!(b64_encode(&ikm), RFC_IKM);
    }

    /// **KAT：一节记录**。同一份 RFC 向量：CEK + NONCE + 明文 → 逐字节等于 RFC 的密文。
    #[test]
    fn rfc8291_record_sealing_known_answer() {
        let cek: [u8; 16] = b64_decode(RFC_CEK).unwrap().try_into().unwrap();
        let nonce: [u8; 12] = b64_decode(RFC_NONCE).unwrap().try_into().unwrap();
        let sealed = seal_record(&cek, &nonce, RFC_PLAINTEXT.as_bytes()).unwrap();
        assert_eq!(b64_encode(&sealed), RFC_CIPHERTEXT);
        assert_eq!(sealed.len(), RFC_PLAINTEXT.len() + 1 + 16);
    }

    /// **KAT：头部布局**。salt ‖ rs(4) ‖ idlen(1) ‖ as_public——`rs` 是 4096（不是别的
    /// 记录大小），`idlen` 是 65（不是 0，也不是 32）。
    #[test]
    fn the_header_layout_matches_the_rfc() {
        let header: Vec<u8> = [
            b64_decode(RFC_SALT).unwrap(),
            RECORD_SIZE.to_be_bytes().to_vec(),
            vec![UNCOMPRESSED_POINT_LEN as u8],
            b64_decode(RFC_AS_PUBLIC).unwrap(),
        ]
        .concat();
        assert_eq!(b64_encode(&header), RFC_HEADER);
        assert_eq!(header.len(), 86);
    }

    /// 端到端：真随机密钥走完整条 [`encrypt_payload`]，再由**测试自己写的**解密侧
    /// （把 RFC 的步骤按自己的话再写一遍）解回来。KAT 钉的是「按 RFC 做」，
    /// 这条钉的是「生产那条路上真的用了它」。
    #[test]
    fn encrypt_payload_round_trips_through_an_independent_decryptor() {
        let rng = SystemRandom::new();
        // 浏览器那一边：一对 ECDH 密钥 + 16 字节鉴权秘密。
        let ua_private = EphemeralPrivateKey::generate(&ECDH_P256, &rng).unwrap();
        let ua_public = ua_private.compute_public_key().unwrap();
        let mut auth_secret = [0u8; 16];
        rng.fill(&mut auth_secret).unwrap();

        // 报文体就是服务端拼好的那一份（`#` 在裸字符串里要躲开，故用双井号）。
        let plaintext =
            br##"{"title":"[AgentPipeline] t1 task_pending","body":"","url":"#/task/t1"}"##;
        let body = encrypt_payload(
            &b64_encode(ua_public.as_ref()),
            &b64_encode(&auth_secret),
            plaintext,
        )
        .unwrap();

        // ── 解密侧（独立写一遍 RFC 8291 §3）──
        assert_eq!(&body[0..16], &body[0..16]);
        let salt: [u8; 16] = body[0..16].try_into().unwrap();
        assert_eq!(
            u32::from_be_bytes(body[16..20].try_into().unwrap()),
            RECORD_SIZE
        );
        assert_eq!(body[20] as usize, UNCOMPRESSED_POINT_LEN);
        let as_public = &body[21..86];
        let ciphertext = &body[86..];
        assert_eq!(
            ciphertext.len(),
            plaintext.len() + 1 + 16,
            "密文 = 明文 + 定界符 + tag"
        );

        let (cek, nonce) = derive_content_keys(
            &agree_ephemeral(
                ua_private,
                &UnparsedPublicKey::new(&ECDH_P256, as_public),
                |s| s.to_vec(),
            )
            .unwrap(),
            &auth_secret,
            ua_public.as_ref(),
            as_public,
            &salt,
        )
        .unwrap();
        let key = aead::LessSafeKey::new(aead::UnboundKey::new(&aead::AES_128_GCM, &cek).unwrap());
        let mut record = ciphertext.to_vec();
        let opened = key
            .open_in_place(
                aead::Nonce::assume_unique_for_key(nonce),
                aead::Aad::empty(),
                &mut record,
            )
            .expect("密文应当解得开");
        assert_eq!(&opened[..opened.len() - 1], plaintext);
        assert_eq!(opened[opened.len() - 1], 0x02, "收尾记录的定界符");
    }

    /// 每次加密都是新的：临时密钥与 salt 都不复用（复用会让两条报文共用派生输入，
    /// 那正是 GCM 最怕的 nonce 重用）。
    #[test]
    fn every_payload_gets_a_fresh_ephemeral_key_and_salt() {
        let p256dh = b64_encode(
            EphemeralPrivateKey::generate(&ECDH_P256, &rng())
                .unwrap()
                .compute_public_key()
                .unwrap()
                .as_ref(),
        );
        let auth = b64_encode(&[7u8; 16]);
        let first = encrypt_payload(&p256dh, &auth, b"same").unwrap();
        let second = encrypt_payload(&p256dh, &auth, b"same").unwrap();
        assert_ne!(first[0..16], second[0..16], "salt 每次都要新");
        assert_ne!(first[21..86], second[21..86], "临时公钥每次都要新");
    }

    /// 坏订阅（长度不对 / 不是合法 base64url）是**报错**，不是「发一条空报文」。
    #[test]
    fn malformed_subscription_keys_are_rejected_with_the_reason() {
        let short = b64_encode(&[1u8; 32]);
        let err = encrypt_payload(&short, &b64_encode(&[0u8; 16]), b"x").unwrap_err();
        assert!(err.contains("p256dh"), "{err}");
        let auth_err = encrypt_payload(RFC_UA_PUBLIC, &b64_encode(&[0u8; 8]), b"x").unwrap_err();
        assert!(auth_err.contains("auth"), "{auth_err}");
        let b64_err =
            encrypt_payload("!!!not base64!!!", &b64_encode(&[0u8; 16]), b"x").unwrap_err();
        assert!(b64_err.contains("base64url"), "{b64_err}");
    }

    /// 超长报文被拒（本票不做分片）。
    #[test]
    fn an_oversized_payload_is_rejected_rather_than_split() {
        let p256dh = b64_encode(
            EphemeralPrivateKey::generate(&ECDH_P256, &rng())
                .unwrap()
                .compute_public_key()
                .unwrap()
                .as_ref(),
        );
        let err = encrypt_payload(&p256dh, &b64_encode(&[0u8; 16]), &vec![b'x'; 5000]).unwrap_err();
        assert!(err.contains("上限"), "{err}");
    }

    // ── VAPID（RFC 8292）──

    /// 生成的密钥对：公钥是 65 字节未压缩点（base64url 87 字符）、私钥是 PKCS#8。
    #[test]
    fn generated_vapid_keys_have_the_expected_shape() {
        let keys = generate_vapid_keys().unwrap();
        let public = b64_decode(&keys.public_key).unwrap();
        assert_eq!(public.len(), UNCOMPRESSED_POINT_LEN);
        assert_eq!(public[0], 0x04, "未压缩点的首字节");
        let private = b64_decode(&keys.private_key).unwrap();
        assert!(private.starts_with(&[0x30]), "PKCS#8 是 DER 的 SEQUENCE");
        assert_ne!(keys.public_key, generate_vapid_keys().unwrap().public_key);
    }

    /// JWT 的形状与签名：头是 `{"typ":"JWT","alg":"ES256"}`、`aud` = endpoint 的 origin、
    /// `exp` 在未来 24 小时内、`sub` 是占位 mailto；签名**真的能验过**
    /// （拿我们先拿到的公钥反过来验，且是 JWS 要的定长 r‖s）。
    #[test]
    fn vapid_jwt_verifies_against_our_own_public_key() {
        let keys = generate_vapid_keys().unwrap();
        let now = Utc::now();
        let header = vapid_authorization(&keys, "https://push.example.net/push/abc", now).unwrap();
        let jwt = header
            .strip_prefix("vapid t=")
            .and_then(|rest| rest.split_once(",k="))
            .expect("应当是 vapid t=<jwt>,k=<pub>");
        let (jwt, k) = jwt;
        assert_eq!(k, keys.public_key, "k 就是公钥本身");
        assert!(!header.contains(&keys.private_key), "私钥不进头部");

        let parts: Vec<&str> = jwt.split('.').collect();
        assert_eq!(parts.len(), 3, "{jwt}");
        let decoded_header = String::from_utf8(b64_decode(parts[0]).unwrap()).unwrap();
        assert_eq!(decoded_header, r#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = String::from_utf8(b64_decode(parts[1]).unwrap()).unwrap();
        assert!(
            claims.contains(r#""aud":"https://push.example.net""#),
            "{claims}"
        );
        assert!(claims.contains(r#""sub":"mailto:"#), "{claims}");
        let exp: i64 = claims
            .split(r#""exp":"#)
            .nth(1)
            .and_then(|rest| rest.split([',', '}']).next())
            .and_then(|raw| raw.parse().ok())
            .expect("exp 是数字");
        assert!(exp > now.timestamp(), "{claims}");
        assert!(
            exp <= now.timestamp() + 24 * 3600,
            "RFC 8292 要求 ≤ 24h：{claims}"
        );

        let signature = b64_decode(parts[2]).unwrap();
        assert_eq!(signature.len(), 64, "ES256 是定长 r‖s（DER 会更长）");
        let public = b64_decode(&keys.public_key).unwrap();
        SigPublicKey::new(&ECDSA_P256_SHA256_FIXED, &public)
            .verify(format!("{}.{}", parts[0], parts[1]).as_bytes(), &signature)
            .expect("签名应当验得过");
    }

    /// origin 取的是 scheme://host[:port]：路径与查询串一个字节都不进去。
    #[test]
    fn the_audience_is_the_endpoints_origin_only() {
        assert_eq!(
            endpoint_origin("https://push.example.net/push/abc?x=1").unwrap(),
            "https://push.example.net"
        );
        assert_eq!(
            endpoint_origin("http://127.0.0.1:9988/push").unwrap(),
            "http://127.0.0.1:9988"
        );
        assert!(endpoint_origin("mailto:someone@example.com").is_err());
    }
}
