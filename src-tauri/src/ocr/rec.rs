//! 识别解码：CTC argmax + 去重去 blank + 字典映射。
//!
//! 对应 Python 参考实现 `scripts/ref_pipeline.py::ctc_decode`。
//!
//! 关键约定（已用实测数据验证，见 docs/DECISIONS.md）：
//! - rec 模型输出**已经是 softmax 概率**（值域 [0,1]，逐帧和为 1），直接 argmax 即可。
//! - blank 固定为索引 0，字符映射为 `chars[index - 1]`。
//! - PP-OCRv6 的 rec 输出类别数 = 字典字符数 + 2，索引 18709 是字典外的富一类，
//!   解码时必须判上界，否则会数组越界或映射到错误字符。

use ndarray::ArrayView2;

/// 解码结果。
#[derive(Debug, Clone)]
pub struct RecResult {
    pub text: String,
    /// 平均置信度（被采纳字符的逐帧概率均值）
    pub score: f64,
}

/// CTC 解码。
///
/// `prob` 形状必须为 `[T, C]`（T 帧，C 类别）。ONNX 的 rec 输出是 `[N, T, C]`，
/// **调用方必须先去掉 batch 维**再传进来——早期版本直接传三维数组并按 `[T, C, ?]`
/// 解读，结果 t 恒等于 1，只能解出单个字符。
///
/// `chars` 为字典字符列表；blank 固定为索引 0，字符映射为 `chars[index - 1]`。
pub fn ctc_decode(prob: &ArrayView2<f32>, chars: &[String]) -> RecResult {
    let t = prob.shape()[0];
    let c = prob.shape()[1];
    if t == 0 || c == 0 {
        return RecResult { text: String::new(), score: 0.0 };
    }

    // 逐帧 argmax
    let mut argmax = vec![0usize; t];
    for i in 0..t {
        let mut best_i = 0usize;
        let mut best_v = f32::MIN;
        for j in 0..c {
            let v = prob[[i, j]];
            if v > best_v {
                best_v = v;
                best_i = j;
            }
        }
        argmax[i] = best_i;
    }

    let mut text = String::new();
    let mut conf_sum = 0.0f64;
    let mut conf_cnt = 0usize;
    let mut prev: usize = usize::MAX; // 用不可能的值作为初始 prev

    for i in 0..t {
        let idx = argmax[i];
        // blank(0) 跳过；与前一帧相同则跳过（CTC 合并重复）
        if idx != 0 && idx != prev {
            // 富一类保护：索引必须落在 [1, len(chars)] 内
            if idx >= 1 && idx <= chars.len() {
                text.push_str(&chars[idx - 1]);
                conf_sum += prob[[i, idx]] as f64;
                conf_cnt += 1;
            }
        }
        prev = idx;
    }

    RecResult {
        text,
        score: if conf_cnt > 0 { conf_sum / conf_cnt as f64 } else { 0.0 },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ndarray::Array2;

    fn chars(n: usize) -> Vec<String> {
        (0..n).map(|i| format!("c{i}")).collect()
    }

    #[test]
    fn ctc_merges_repeats_and_skips_blank() {
        // 字典 3 个字符 -> 合法索引 1,2,3；blank 为 0
        let dict = chars(3);
        let mut a = Array2::<f32>::zeros((5, 4));
        // 帧序列: 1, 1, 0, 2, 2 -> 合并重复 + 去 blank 后应得 "c0c1"
        // （索引 1 -> chars[0]，索引 2 -> chars[1]）
        a[[0, 1]] = 1.0;
        a[[1, 1]] = 1.0;
        a[[2, 0]] = 1.0;
        a[[3, 2]] = 1.0;
        a[[4, 2]] = 1.0;
        let r = ctc_decode(&a.view(), &dict);
        assert_eq!(r.text, "c0c1");
        assert!(r.score > 0.99);
    }

    #[test]
    fn ctc_out_of_range_index_is_ignored() {
        // 字典 3 个字符，类别数 4 -> 索引 0..3。
        // 索引 3 是字典边界；索引 4 及以后才是真正越界，必须被忽略而不是 panic。
        let dict = chars(3);
        let mut a = Array2::<f32>::zeros((2, 6));
        a[[0, 4]] = 1.0; // 越界
        a[[1, 5]] = 1.0; // 越界
        let r = ctc_decode(&a.view(), &dict);
        assert_eq!(r.text, "");
    }

    #[test]
    fn ctc_realistic_v6_dimensions() {
        // 复刻真实规模：18708 字典 + blank + 1 富一类 = 18710 类，40 帧
        let n_chars = 18708usize;
        let dict: Vec<String> = (0..n_chars).map(|i| format!("x{i}")).collect();
        let mut a = Array2::<f32>::zeros((40, 18710));
        // 前 5 帧给出合法字符，第 6 帧给出富一类索引 18709（必须被忽略）
        for (t, idx) in [1usize, 1, 0, 2, 2].iter().enumerate() {
            a[[t, *idx]] = 1.0;
        }
        a[[5, 18709]] = 1.0;
        let r = ctc_decode(&a.view(), &dict);
        assert_eq!(r.text, "x0x1");
    }

    #[test]
    fn ctc_empty_input() {
        let dict = chars(3);
        let a = Array2::<f32>::zeros((0, 4));
        let r = ctc_decode(&a.view(), &dict);
        assert_eq!(r.text, "");
        assert_eq!(r.score, 0.0);
    }
}
