# Implementation Plan

1. 在完整候选扫描后发布经优先级裁决的 CATA 依赖清单，生产 locator 只消费该清单。
2. 令闭包缓存同时校验源窗口右端和依赖文件指纹，并把缓存发布延后到窗口提交成功之后。
3. 在暂存窗口内用 replacement semantics 写 CATA 元素；依赖错误和 missing 直接上浮。
4. 增加依赖进度、300 秒无进展 watchdog、任务/健康字段及 epoch 延后安装。
5. 增加纯测试、隔离 live 测试、几何摘要对拍和证据台账。
