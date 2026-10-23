//! 中文提示词 → 英文提示词。
//!
//! 为什么需要这一层
//! ----------------
//! 语言找图的文本塔是 **MobileCLIP2-B**，它和它的 BPE 词表都是英文语义空间。
//! 中文直接喂进去会被拆成 UTF-8 字节片，那些 byte token 在英文语料里几乎没被
//! 训练过 —— 实测（2026-10-23，`assets/bus.png`，阈值压到 0.10）：
//!
//! ```text
//! 人       → 0.569    （英文 person 是 0.913）
//! 公交车    → 0.000    （英文 bus 是 0.879）
//! 大巴/车/轮子/窗户/狗/手机/椅子 … 16 个词 → 全 0
//! ```
//!
//! 也就是说中文不是「完全无效」而是**碰运气**：只有个别单字偶然撞上，分数还只有
//! 英文的一半。这种状态比干脆报错更危险 —— 用户会以为它能用。
//!
//! 换中文文本塔（CN-CLIP 之类）**原理上不行**：它的特征空间和 YOLOE 的
//! `texthead.onnx` 不对齐，喂进去只会输出垃圾，要能用必须重训对齐投影头。
//! 所以唯一能立刻落地的路子就是**先把中文映射成英文，再走原来的链路**。
//!
//! 这是**工程代价问题**（词表要写多少条），不是原理天花板：词表覆盖到的中文
//! 场景，效果和直接写英文完全一样。
//!
//! 用户自定义
//! ----------
//! `models/yoloe/zh_words.json`（找不到就用内置表）：
//!
//! ```json
//! { "公交车": "bus", "我的提交按钮": "submit button" }
//! ```
//!
//! 用户写的**覆盖**内置表（同一个中文词以用户的为准）。

use std::collections::HashMap;
use std::path::Path;

/// 内置映射。左边中文、右边送给文本塔的英文提示词。
///
/// 覆盖三块：COCO 80 类（YOLOE 原生类别，效果最好）、常用界面元素、
/// 日常物体。英文侧一律用 CLIP 友好的小写名词短语（多词用空格）。
pub const BUILTIN: &[(&str, &str)] = &[
    // --- COCO 80 类 -------------------------------------------------------
    ("人", "person"),
    ("人物", "person"),
    ("行人", "person"),
    ("小孩", "child"),
    ("男人", "man"),
    ("女人", "woman"),
    ("自行车", "bicycle"),
    ("单车", "bicycle"),
    ("汽车", "car"),
    ("轿车", "car"),
    ("小汽车", "car"),
    ("摩托车", "motorcycle"),
    ("电动车", "motorcycle"),
    ("飞机", "airplane"),
    ("公交车", "bus"),
    ("巴士", "bus"),
    ("大巴", "bus"),
    ("公共汽车", "bus"),
    ("火车", "train"),
    ("高铁", "train"),
    ("地铁", "train"),
    ("卡车", "truck"),
    ("货车", "truck"),
    ("船", "boat"),
    ("轮船", "boat"),
    ("红绿灯", "traffic light"),
    ("信号灯", "traffic light"),
    ("消防栓", "fire hydrant"),
    ("路牌", "stop sign"),
    ("停车标志", "stop sign"),
    ("停车计时器", "parking meter"),
    ("长椅", "bench"),
    ("椅子", "chair"),
    ("座椅", "chair"),
    ("鸟", "bird"),
    ("猫", "cat"),
    ("狗", "dog"),
    ("小狗", "dog"),
    ("马", "horse"),
    ("羊", "sheep"),
    ("牛", "cow"),
    ("大象", "elephant"),
    ("熊", "bear"),
    ("斑马", "zebra"),
    ("长颈鹿", "giraffe"),
    ("背包", "backpack"),
    ("双肩包", "backpack"),
    ("雨伞", "umbrella"),
    ("伞", "umbrella"),
    ("手提包", "handbag"),
    ("包", "bag"),
    ("领带", "tie"),
    ("行李箱", "suitcase"),
    ("拉杆箱", "suitcase"),
    ("飞盘", "frisbee"),
    ("滑雪板", "skis"),
    ("单板滑雪", "snowboard"),
    ("球", "sports ball"),
    ("篮球", "basketball"),
    ("足球", "soccer ball"),
    ("网球", "tennis ball"),
    ("乒乓球", "ping pong ball"),
    ("风筝", "kite"),
    ("棒球棒", "baseball bat"),
    ("棒球手套", "baseball glove"),
    ("滑板", "skateboard"),
    ("冲浪板", "surfboard"),
    ("网球拍", "tennis racket"),
    ("瓶子", "bottle"),
    ("水杯", "cup"),
    ("杯子", "cup"),
    ("玻璃杯", "wine glass"),
    ("酒杯", "wine glass"),
    ("叉子", "fork"),
    ("刀", "knife"),
    ("勺子", "spoon"),
    ("碗", "bowl"),
    ("香蕉", "banana"),
    ("苹果", "apple"),
    ("三明治", "sandwich"),
    ("汉堡", "hamburger"),
    ("橙子", "orange"),
    ("橘子", "orange"),
    ("西兰花", "broccoli"),
    ("胡萝卜", "carrot"),
    ("热狗", "hot dog"),
    ("披萨", "pizza"),
    ("甜甜圈", "donut"),
    ("蛋糕", "cake"),
    ("沙发", "couch"),
    ("盆栽", "potted plant"),
    ("植物", "plant"),
    ("树", "tree"),
    ("花", "flower"),
    ("床", "bed"),
    ("餐桌", "dining table"),
    ("桌子", "table"),
    ("马桶", "toilet"),
    ("电视", "tv"),
    ("电视机", "tv"),
    ("笔记本电脑", "laptop"),
    ("电脑", "computer"),
    ("显示器", "monitor"),
    ("鼠标", "mouse"),
    ("遥控器", "remote"),
    ("键盘", "keyboard"),
    ("手机", "cell phone"),
    ("微波炉", "microwave"),
    ("烤箱", "oven"),
    ("烤面包机", "toaster"),
    ("水槽", "sink"),
    ("水池", "sink"),
    ("冰箱", "refrigerator"),
    ("书", "book"),
    ("书本", "book"),
    ("时钟", "clock"),
    ("钟", "clock"),
    ("手表", "watch"),
    ("花瓶", "vase"),
    ("剪刀", "scissors"),
    ("玩偶", "teddy bear"),
    ("泰迪熊", "teddy bear"),
    ("吹风机", "hair drier"),
    ("牙刷", "toothbrush"),
    // --- 界面元素（不是 COCO，但 YOLOE 是开放词汇，找得到） ----------------
    ("按钮", "button"),
    ("图标", "icon"),
    ("菜单", "menu"),
    ("输入框", "input box"),
    ("文本框", "text field"),
    ("搜索框", "search box"),
    ("复选框", "checkbox"),
    ("单选框", "radio button"),
    ("下拉框", "dropdown"),
    ("下拉菜单", "dropdown menu"),
    ("开关", "toggle switch"),
    ("滑块", "slider"),
    ("进度条", "progress bar"),
    ("滚动条", "scrollbar"),
    ("标签", "label"),
    ("标题", "title"),
    ("文字", "text"),
    ("数字", "number"),
    ("日期", "date"),
    ("价格", "price"),
    ("弹窗", "dialog"),
    ("对话框", "dialog box"),
    ("提示框", "tooltip"),
    ("窗口", "window"),
    ("工具栏", "toolbar"),
    ("侧边栏", "sidebar"),
    ("导航栏", "navigation bar"),
    ("标签页", "tab"),
    ("卡片", "card"),
    ("头像", "avatar"),
    ("链接", "link"),
    ("图片", "image"),
    ("照片", "photo"),
    ("二维码", "qr code"),
    ("条形码", "barcode"),
    ("表格", "table"),
    ("列表", "list"),
    ("关闭按钮", "close button"),
    ("登录按钮", "login button"),
    ("提交按钮", "submit button"),
    ("确定按钮", "ok button"),
    ("取消按钮", "cancel button"),
    ("首页", "home page"),
    ("设置", "settings"),
    ("验证码", "captcha"),
    ("输入框光标", "text cursor"),
    // --- 日常物体 ---------------------------------------------------------
    ("车牌", "license plate"),
    ("车轮", "wheel"),
    ("轮胎", "tire"),
    ("车灯", "headlight"),
    ("后视镜", "rearview mirror"),
    ("方向盘", "steering wheel"),
    ("门", "door"),
    ("窗户", "window"),
    ("玻璃", "glass"),
    ("墙", "wall"),
    ("地板", "floor"),
    ("天花板", "ceiling"),
    ("楼梯", "stairs"),
    ("电梯", "elevator"),
    ("屋顶", "roof"),
    ("房子", "house"),
    ("楼", "building"),
    ("建筑物", "building"),
    ("桥", "bridge"),
    ("路", "road"),
    ("公路", "road"),
    ("街道", "street"),
    ("山", "mountain"),
    ("河", "river"),
    ("海", "sea"),
    ("湖", "lake"),
    ("云", "cloud"),
    ("天空", "sky"),
    ("太阳", "sun"),
    ("月亮", "moon"),
    ("星星", "star"),
    ("雪", "snow"),
    ("雨", "rain"),
    ("草", "grass"),
    ("石头", "rock"),
    ("沙子", "sand"),
    ("灯", "lamp"),
    ("灯泡", "light bulb"),
    ("台灯", "desk lamp"),
    ("空调", "air conditioner"),
    ("风扇", "fan"),
    ("插座", "power outlet"),
    ("电池", "battery"),
    ("充电器", "charger"),
    ("数据线", "usb cable"),
    ("耳机", "headphones"),
    ("音响", "speaker"),
    ("摄像头", "camera"),
    ("镜子", "mirror"),
    ("笔", "pen"),
    ("铅笔", "pencil"),
    ("纸", "paper"),
    ("文件", "document"),
    ("文件夹", "folder"),
    ("发票", "invoice"),
    ("收据", "receipt"),
    ("名片", "business card"),
    ("身份证", "id card"),
    ("银行卡", "bank card"),
    ("护照", "passport"),
    ("合同", "contract"),
    ("快递", "package"),
    ("包裹", "parcel"),
    ("箱子", "box"),
    ("盒子", "box"),
    ("袋子", "plastic bag"),
    ("钥匙", "key"),
    ("钱包", "wallet"),
    ("眼镜", "glasses"),
    ("帽子", "hat"),
    ("鞋", "shoe"),
    ("袜子", "sock"),
    ("衣服", "clothing"),
    ("裤子", "pants"),
    ("裙子", "skirt"),
    ("外套", "coat"),
    ("手套", "glove"),
    ("毛巾", "towel"),
    ("食物", "food"),
    ("水果", "fruit"),
    ("蔬菜", "vegetable"),
    ("米饭", "rice"),
    ("面条", "noodles"),
    ("饮料", "drink"),
    ("咖啡", "coffee"),
    ("茶", "tea"),
    ("烟", "cigarette"),
    ("药", "medicine"),
    ("口罩", "face mask"),
    ("锤子", "hammer"),
    ("螺丝刀", "screwdriver"),
    ("工具", "tool"),
    ("枪", "gun"),
    ("玩具", "toy"),
    ("鱼", "fish"),
    ("鸡", "chicken"),
    ("鸭", "duck"),
    ("猪", "pig"),
    ("兔子", "rabbit"),
    ("老鼠", "mouse"),
    ("虫子", "insect"),
    ("蜘蛛", "spider"),
    ("火焰", "fire"),
    ("烟雾", "smoke"),
];

/// 一个词解析后的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// 用户看到的类名（写回 `class_name`，保持用户输入的中文）。
    pub display: String,
    /// 真正送进文本塔的英文提示词。
    pub clip: String,
}

/// 判断一个词是否需要查表（含非 ASCII 字符就当中文处理）。
fn needs_map(w: &str) -> bool {
    !w.is_ascii()
}

/// 读用户自定义映射。文件不存在 / 格式不对就当没有（不阻断识别）。
pub fn load_extra(path: Option<&Path>) -> HashMap<String, String> {
    let Some(p) = path else {
        return HashMap::new();
    };
    let Ok(text) = std::fs::read_to_string(p) else {
        return HashMap::new();
    };
    match serde_json::from_str::<HashMap<String, String>>(&text) {
        Ok(m) => m,
        Err(e) => {
            tracing::warn!("中文词表 {} 解析失败，忽略：{e}", p.display());
            HashMap::new()
        }
    }
}

/// 查一个中文词的英文对应。用户表优先于内置表。
pub fn lookup(word: &str, extra: &HashMap<String, String>) -> Option<String> {
    if let Some(v) = extra.get(word) {
        let v = v.trim();
        if !v.is_empty() {
            return Some(v.to_string());
        }
    }
    BUILTIN
        .iter()
        .find(|(zh, _)| *zh == word)
        .map(|(_, en)| (*en).to_string())
}

/// 给未命中的中文词找相近的已知词：按**共同字符数**排序取前 3。
///
/// 目的不是模糊匹配，而是让报错能告诉调用方（人或 AI）「换个什么词能work」。
pub fn suggest(word: &str, extra: &HashMap<String, String>) -> Vec<String> {
    let chars: Vec<char> = word.chars().collect();
    let mut scored: Vec<(usize, String)> = BUILTIN
        .iter()
        .map(|(zh, _)| *zh)
        .chain(extra.keys().map(|k| k.as_str()))
        .map(|zh| {
            let hit = chars.iter().filter(|c| zh.contains(**c)).count();
            (hit, zh.to_string())
        })
        .filter(|(hit, _)| *hit > 0)
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.truncate(3);
    scored.into_iter().map(|(_, s)| s).collect()
}

/// 把一批提示词解析成「显示名 + 文本塔用词」。
///
/// 英文（纯 ASCII）原样放行 —— 内置的开放词汇能力不该被词表限制。
/// 中文查表；查不到就记进 `Err`，由调用方拼成可读的报错。
pub fn resolve(
    words: &[String],
    extra: &HashMap<String, String>,
) -> Result<Vec<Resolved>, Vec<String>> {
    let mut out = Vec::with_capacity(words.len());
    let mut missing = Vec::new();
    for w in words {
        let w = w.trim();
        if !needs_map(w) {
            out.push(Resolved {
                display: w.to_string(),
                clip: w.to_string(),
            });
            continue;
        }
        match lookup(w, extra) {
            Some(en) => out.push(Resolved {
                display: w.to_string(),
                clip: en,
            }),
            None => missing.push(w.to_string()),
        }
    }
    if missing.is_empty() {
        Ok(out)
    } else {
        Err(missing)
    }
}

/// 未命中时的一句话建议（给 BadRequest 拼文案用）。
pub fn missing_message(missing: &[String], extra: &HashMap<String, String>) -> String {
    let mut parts = Vec::new();
    for m in missing {
        let near = suggest(m, extra);
        let near = if near.is_empty() {
            String::new()
        } else {
            format!("，相近的中文词有：{}", near.join("、"))
        };
        parts.push(format!("「{m}」{near}"));
    }
    format!(
        "这些词不在内置中文词表里：{}。语言找图的文本塔是英文语义空间，\
         中文要先映射成英文；可以直接写英文（如 bus、person），\
         也可以把映射写进 models/yoloe/zh_words.json（如 {{\"公交车\": \"bus\"}}）后重启服务。",
        parts.join("；")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_passes_through() {
        let extra = HashMap::new();
        let got = resolve(&["bus".into(), "person".into()], &extra).unwrap();
        assert_eq!(got[0].clip, "bus");
        assert_eq!(got[0].display, "bus");
        assert_eq!(got[1].clip, "person");
    }

    #[test]
    fn chinese_maps_to_english_but_keeps_display() {
        let extra = HashMap::new();
        let got = resolve(&["公交车".into(), "人".into()], &extra).unwrap();
        assert_eq!(got[0].clip, "bus");
        assert_eq!(got[0].display, "公交车");
        assert_eq!(got[1].clip, "person");
        assert_eq!(got[1].display, "人");
    }

    #[test]
    fn mixed_zh_en() {
        let extra = HashMap::new();
        let got = resolve(&["人".into(), "bus".into()], &extra).unwrap();
        assert_eq!(got[0].clip, "person");
        assert_eq!(got[1].clip, "bus");
    }

    #[test]
    fn user_extra_overrides_builtin() {
        let mut extra = HashMap::new();
        extra.insert("公交车".into(), "school bus".into());
        let got = resolve(&["公交车".into()], &extra).unwrap();
        assert_eq!(got[0].clip, "school bus");
    }

    #[test]
    fn unknown_chinese_reports_with_suggestion() {
        let extra = HashMap::new();
        let err = resolve(&["宇宙飞船".into()], &extra).unwrap_err();
        assert_eq!(err, vec!["宇宙飞船".to_string()]);
        let msg = missing_message(&err, &extra);
        assert!(msg.contains("宇宙飞船"));
        assert!(msg.contains("zh_words.json"));
    }

    #[test]
    fn builtin_has_no_duplicate_keys() {
        let mut seen: HashMap<&str, &str> = HashMap::new();
        for (zh, en) in BUILTIN {
            if let Some(prev) = seen.insert(zh, en) {
                panic!("内置中文词表有重复键 {zh}：{prev} 与 {en}");
            }
            assert!(!zh.is_empty() && !en.is_empty(), "空键或空值：{zh:?}");
        }
    }
}
