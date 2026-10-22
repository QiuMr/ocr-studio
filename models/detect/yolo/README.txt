目标检测模型目录
================

把你的 .onnx 检测模型放进这个目录，程序会自动扫描到，不用改配置。

  - 目录里**每一个** *.onnx 都是一个可选的检测引擎，下拉里按文件名区分。
  - 列表里的第一个是「默认模型」（engine 参数不写 id 时用它）。
  - 也可以在「设置」页点「添加模型」，指向任意目录的 onnx（只记路径，不复制文件）。
  - 类别名自动从模型里读（ultralytics 导出会写入 metadata），不用手写 labels.txt；
    读不到时才回落到同目录的 labels.txt。

本目录**出厂为空**：这是你自己的模型工作台，不预置任何模型。
没有模型时 /vision/detect 会明确提示「没有找到检测模型」，不会静默返回空。

支持：单输出、轴对齐的检测模型（v5 / v6 / v7 / v8 / v10 / v11 / v12 / v26，
以及 YOLOE 开放词汇模型；含 NMS-free 端到端导出）。
分割（segment）模型只取检测头（掩码系数列会被跳过），所以 YOLOE-26 这种
官方只放 -seg 权重的情况也能用。
不支持：v3/v4 三尺度多输出 head、姿态（pose）/ 旋转框（obb）/ 分类（classify）
模型（会明确报错并说明原因）。

导出示例（ultralytics）：
  yolo export model=my.pt format=onnx opset=12 imgsz=640 simplify=True

想直接拿一个现成的语义找图模型（YOLOE-26，按参考图导出）：
  python scripts/fetch_yoloe.py --image assets/bus.png --box 30,230,800,776 --name bus
