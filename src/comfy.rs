//! 从 ComfyUI 生成的 PNG 里读出正/负向提示词（ADR-0013）。
//!
//! 只做一件事：PNG 里的 `prompt` 文本块（ComfyUI 把整张节点图存进去）
//! 被解析成一棵节点图，再顺着采样器的 positive / negative 连线找回
//! `CLIPTextEncode` 节点的文本。
//!
//! 有意不做的：解码像素、校验 CRC、解压 zTXt、认 A1111 的 `parameters`、
//! 读 EXIF/WebP。这些格式不在本程序的使用场景里，宁可当普通图片。

use serde_json::{Map, Value};

/// 一次检测的结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Detection {
    /// 不是 ComfyUI 的 PNG：当普通图片处理，不提示。
    NotComfyUi,
    /// 是 ComfyUI 的图，但没找到可用的正向提示词（含只有负向的情况）。
    NoPositive,
    /// 找到了：两段正文已 trim，`negative` 可能为空。
    Found { positive: String, negative: String },
}

/// 检测图片字节里是否带着 ComfyUI 的提示词。
///
/// 纯函数、不碰文件系统，几微秒量级；调用方负责读文件与入库。
pub fn detect(bytes: &[u8]) -> Detection {
    let Some(json) = find_prompt_json(bytes) else {
        return Detection::NotComfyUi;
    };
    let Ok(graph) = serde_json::from_str::<Value>(&json) else {
        return Detection::NotComfyUi;
    };
    match extract_prompts(&graph) {
        Some((positive, negative)) => Detection::Found {
            positive,
            negative,
        },
        None if has_class_types(&graph) => Detection::NoPositive,
        // 有 `prompt` 块但里面没有节点图：别的程序写的，当普通图片。
        None => Detection::NotComfyUi,
    }
}

const PNG_SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
/// 跟随连线的上限；真实节点图最多十几跳，这个数只用来防环。
const MAX_HOPS: usize = 32;
/// 采样器上找 positive / negative 的类名前缀。
const SAMPLER_PREFIXES: [&str; 2] = ["KSampler", "SamplerCustom"];

/// 从 PNG 块头里取出 `prompt` 文本块的内容。
///
/// 只扫块头：长度 4B 大端 | 类型 4B | 数据 | CRC 4B，遇到 IEND 停。
fn find_prompt_json(bytes: &[u8]) -> Option<String> {
    if bytes.len() < PNG_SIGNATURE.len() || bytes[..8] != PNG_SIGNATURE {
        return None;
    }
    let mut pos = PNG_SIGNATURE.len();
    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let kind = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start.checked_add(len)?;
        // 块被截断（或被伪造的长度骗了）：不再往下读。
        if data_end + 4 > bytes.len() {
            return None;
        }
        let data = &bytes[data_start..data_end];
        match kind {
            b"IEND" => return None,
            b"tEXt" => {
                if let Some((key, value)) = split_text(data) {
                    if key == "prompt" {
                        return Some(value.to_string());
                    }
                }
            }
            b"iTXt" => {
                if let Some((key, value)) = parse_itxt(data) {
                    if key == "prompt" {
                        return Some(value);
                    }
                }
            }
            _ => {}
        }
        pos = data_end + 4;
    }
    None
}

/// `tEXt`：`keyword\0text`。
fn split_text(data: &[u8]) -> Option<(&str, &str)> {
    let sep = data.iter().position(|b| *b == 0)?;
    let key = std::str::from_utf8(&data[..sep]).ok()?;
    let value = std::str::from_utf8(&data[sep + 1..]).ok()?;
    Some((key, value))
}

/// `iTXt`：`keyword\0 flag method lang\0 translated\0 text`。
///
/// 压缩过的（flag = 1）直接跳过——ComfyUI 不写压缩的 iTXt，为此引入 inflate 不值得。
fn parse_itxt(data: &[u8]) -> Option<(&str, String)> {
    let key_end = data.iter().position(|b| *b == 0)?;
    let key = std::str::from_utf8(&data[..key_end]).ok()?;
    let rest = data.get(key_end + 1..)?;
    let compressed = *rest.first()? != 0;
    if compressed {
        return None;
    }
    let mut rest = rest.get(2..)?;
    for _ in 0..2 {
        let end = rest.iter().position(|b| *b == 0)?;
        rest = rest.get(end + 1..)?;
    }
    Some((key, String::from_utf8_lossy(rest).to_string()))
}

fn has_class_types(graph: &Value) -> bool {
    graph
        .as_object()
        .map(|nodes| {
            nodes
                .values()
                .any(|node| node.get("class_type").and_then(Value::as_str).is_some())
        })
        .unwrap_or(false)
}

/// 在节点图里找第一个能给出正向前提示词的采样器。
fn extract_prompts(graph: &Value) -> Option<(String, String)> {
    let nodes = graph.as_object()?;
    let graph = Graph { nodes };
    for node in nodes.values() {
        let Some((positive, negative)) = graph.sampler_refs(node) else {
            continue;
        };
        let Some(positive) = graph.resolve_text(positive, 0).filter(|t| !t.is_empty()) else {
            continue;
        };
        let negative = negative
            .and_then(|value| graph.resolve_text(value, 0))
            .unwrap_or_default();
        return Some((positive, negative));
    }
    None
}

struct Graph<'a> {
    nodes: &'a Map<String, Value>,
}

impl<'a> Graph<'a> {
    /// 采样器节点上的 (positive, negative) 连线；不是采样器返回 `None`。
    fn sampler_refs(&self, node: &'a Value) -> Option<(&'a Value, Option<&'a Value>)> {
        let class = node.get("class_type")?.as_str()?;
        if SAMPLER_PREFIXES.iter().any(|p| class.starts_with(p)) {
            let inputs = node.get("inputs")?;
            // SamplerCustomAdvanced 把条件藏在一层 guider 后面，negative 不一定存在。
            if class.starts_with("SamplerCustom") && inputs.get("positive").is_none() {
                let guider = self.follow(inputs.get("guider")?)?;
                let inputs = guider.get("inputs")?;
                if inputs.get("negative").is_some() {
                    return Some((inputs.get("positive")?, inputs.get("negative")));
                }
                return Some((inputs.get("positive")?, None));
            }
            return Some((inputs.get("positive")?, inputs.get("negative")));
        }
        None
    }

    /// 把一条连线解析成节点。
    fn follow(&self, value: &Value) -> Option<&'a Value> {
        let id = link_target(value)?;
        self.nodes.get(&id)
    }

    /// 把一条连线解析成文本：字符串直接返回，连线递归跟随。
    fn resolve_text(&self, value: &'a Value, depth: usize) -> Option<String> {
        if depth > MAX_HOPS {
            return None;
        }
        if let Some(text) = value.as_str() {
            return Some(text.trim().to_string());
        }
        let node = self.follow(value)?;
        let class = node.get("class_type")?.as_str()?;
        if class.starts_with("CLIPTextEncode") {
            // 普通 CLIPTextEncode 用 text；SDXL 的一对节点用 text_g / text_l。
            for key in ["text", "text_g", "text_l"] {
                let Some(text) = node.get("inputs")?.get(key) else {
                    continue;
                };
                if let Some(text) = self.resolve_text(text, depth + 1) {
                    if !text.is_empty() {
                        return Some(text);
                    }
                }
            }
            return None;
        }
        // 直通节点（Reroute / 条件合并 / 强化）：只有一条输入连线时才跟过去，
        // 多输入的节点猜不准该走哪条。
        let inputs = node.get("inputs")?.as_object()?;
        let links: Vec<&Value> = inputs.values().filter(|v| link_target(v).is_some()).collect();
        if links.len() == 1 {
            return self.resolve_text(links[0], depth + 1);
        }
        None
    }
}

/// 连线是 `[节点 id, 输出槽位]`；id 在 ComfyUI 里是字符串，兼容数字写法。
fn link_target(value: &Value) -> Option<String> {
    let array = value.as_array()?;
    match array.first()? {
        Value::String(id) => Some(id.clone()),
        Value::Number(id) => Some(id.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(data);
        out.extend_from_slice(&[0, 0, 0, 0]); // CRC 不参与判断
        out
    }

    fn png(chunks: Vec<Vec<u8>>) -> Vec<u8> {
        let mut out = PNG_SIGNATURE.to_vec();
        for chunk in chunks {
            out.extend_from_slice(&chunk);
        }
        out.extend_from_slice(&chunk(b"IEND", &[]));
        out
    }

    fn text_chunk(key: &str, value: &str) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend_from_slice(key.as_bytes());
        data.push(0);
        data.extend_from_slice(value.as_bytes());
        chunk(b"tEXt", &data)
    }

    fn comfy_png(graph: &str) -> Vec<u8> {
        png(vec![
            chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]),
            text_chunk("prompt", graph),
        ])
    }

    fn found(detection: Detection) -> (String, String) {
        match detection {
            Detection::Found {
                positive,
                negative,
            } => (positive, negative),
            other => panic!("应识别为 Found，实际是 {other:?}"),
        }
    }

    #[test]
    fn plain_ksampler_with_literal_texts() {
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {
                "positive": ["6", 0], "negative": ["7", 0], "seed": 1}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "  1girl, solo  "}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "lowres"}}
        }"#;
        assert_eq!(
            found(detect(&comfy_png(graph))),
            ("1girl, solo".to_string(), "lowres".to_string())
        );
    }

    #[test]
    fn node_ids_with_colons_still_resolve() {
        let graph = r#"{
            "60:19": {"class_type": "KSampler", "inputs": {
                "positive": ["60:11", 0], "negative": ["60:12", 0]}},
            "60:11": {"class_type": "CLIPTextEncode", "inputs": {"text": "masterpiece"}},
            "60:12": {"class_type": "CLIPTextEncode", "inputs": {"text": "noisy"}}
        }"#;
        assert_eq!(
            found(detect(&comfy_png(graph))),
            ("masterpiece".to_string(), "noisy".to_string())
        );
    }

    #[test]
    fn sampler_custom_advanced_reads_through_cfg_guider() {
        let graph = r#"{
            "5": {"class_type": "SamplerCustomAdvanced", "inputs": {
                "guider": ["9", 0], "sampler": ["10", 0]}},
            "9": {"class_type": "CFGGuider", "inputs": {
                "positive": ["6", 0], "negative": ["7", 0], "cfg": 5.0}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "cat"}},
            "7": {"class_type": "CLIPTextEncode", "inputs": {"text": "dog"}}
        }"#;
        assert_eq!(
            found(detect(&comfy_png(graph))),
            ("cat".to_string(), "dog".to_string())
        );
    }

    #[test]
    fn basic_guider_has_no_negative() {
        let graph = r#"{
            "5": {"class_type": "SamplerCustomAdvanced", "inputs": {"guider": ["9", 0]}},
            "9": {"class_type": "BasicGuider", "inputs": {"positive": ["6", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "cat"}}
        }"#;
        assert_eq!(
            found(detect(&comfy_png(graph))),
            ("cat".to_string(), String::new())
        );
    }

    #[test]
    fn passthrough_node_is_followed() {
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["8", 0]}},
            "8": {"class_type": "ConditioningSetArea", "inputs": {
                "conditioning": ["6", 0], "width": 64}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "cat"}}
        }"#;
        assert_eq!(found(detect(&comfy_png(graph))).0, "cat");
    }

    #[test]
    fn sdxl_prefers_text_g_and_falls_back_to_text_l() {
        let both = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["6", 0]}},
            "6": {"class_type": "CLIPTextEncodeSDXL", "inputs": {
                "text_g": "global text", "text_l": "local text"}}
        }"#;
        assert_eq!(found(detect(&comfy_png(both))).0, "global text");

        let only_local = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["6", 0]}},
            "6": {"class_type": "CLIPTextEncodeSDXL", "inputs": {"text_l": "local text"}}
        }"#;
        assert_eq!(found(detect(&comfy_png(only_local))).0, "local text");
    }

    #[test]
    fn bypassed_positive_node_yields_no_positive() {
        // 正向节点被旁路：连线指向一个不存在的节点。
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["99", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "orphan"}}
        }"#;
        assert_eq!(detect(&comfy_png(graph)), Detection::NoPositive);
    }

    #[test]
    fn cyclic_graph_terminates() {
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["8", 0]}},
            "8": {"class_type": "Reroute", "inputs": {"": ["9", 0]}},
            "9": {"class_type": "Reroute", "inputs": {"": ["8", 0]}}
        }"#;
        assert_eq!(detect(&comfy_png(graph)), Detection::NoPositive);
    }

    #[test]
    fn empty_positive_text_is_no_positive() {
        let graph = r#"{
            "3": {"class_type": "KSampler", "inputs": {"positive": ["6", 0]}},
            "6": {"class_type": "CLIPTextEncode", "inputs": {"text": "   "}}
        }"#;
        assert_eq!(detect(&comfy_png(graph)), Detection::NoPositive);
    }

    #[test]
    fn non_png_and_missing_prompt_chunk_are_not_comfyui() {
        let jpeg = [0xFF, 0xD8, 0xFF, 0xE0, 0, 0, 0, 0];
        assert_eq!(detect(&jpeg), Detection::NotComfyUi);
        assert_eq!(detect(&[]), Detection::NotComfyUi);

        // 只有 workflow 块（ComfyUI 也会写，但不是提示词来源）。
        let only_workflow = png(vec![text_chunk("workflow", "{}")]);
        assert_eq!(detect(&only_workflow), Detection::NotComfyUi);
    }

    #[test]
    fn prompt_chunk_without_node_graph_is_not_comfyui() {
        let bytes = png(vec![text_chunk("prompt", r#"{"hello": "world"}"#)]);
        assert_eq!(detect(&bytes), Detection::NotComfyUi);
        let not_json = png(vec![text_chunk("prompt", "not json at all")]);
        assert_eq!(detect(&not_json), Detection::NotComfyUi);
    }

    #[test]
    fn truncated_chunk_does_not_panic() {
        let mut bytes = comfy_png(r#"{"3": {"class_type": "KSampler"}}"#);
        bytes.truncate(bytes.len() - 6);
        // 不 panic 即可；结果只允许是 NotComfyUi 或 NoPositive。
        assert!(matches!(
            detect(&bytes),
            Detection::NotComfyUi | Detection::NoPositive
        ));
    }

    #[test]
    fn compressed_itxt_is_skipped() {
        let mut data = Vec::new();
        data.extend_from_slice(b"prompt");
        data.push(0);
        data.push(1); // compressed flag
        data.push(0);
        data.extend_from_slice(b"\0\0");
        data.extend_from_slice(b"garbage");
        let bytes = png(vec![chunk(b"iTXt", &data)]);
        assert_eq!(detect(&bytes), Detection::NotComfyUi);
    }

    #[test]
    fn uncompressed_itxt_is_read() {
        let mut data = Vec::new();
        data.extend_from_slice(b"prompt");
        data.push(0);
        data.push(0); // 未压缩
        data.push(0);
        data.extend_from_slice(b"\0\0");
        data.extend_from_slice(
            br#"{"3":{"class_type":"KSampler","inputs":{"positive":["6",0]}},
                 "6":{"class_type":"CLIPTextEncode","inputs":{"text":"from itxt"}}}"#,
        );
        let bytes = png(vec![chunk(b"iTXt", &data)]);
        assert_eq!(found(detect(&bytes)).0, "from itxt");
    }
}
