# V001 — 浏览器劫持探索与签到设备隔离

> 让多个 TRAE 桌面账号在**同一台 Windows** 上各自独立签到，互不撞车。
> 当前**已走通的是「免劫持」方案**（重点）；浏览器劫持是早期尝试、**未走通**，仅按应急预案保留，见文末。

---

## 0. 一目速览

| 关键问题 | 一句话答案 |
| --- | --- |
| 为什么必须隔离设备 | 服务端按「设备 × 账号」判重，一天一台设备只能给一个账号签一次 |
| 现在怎么做 | **免劫持**：客户端机器码登录一次；签到时若目录设备撞了主账号机器码，就自动换独立设备码 |
| 真正隔离靠什么 | `get_or_create_checkin_device_id` 的「撞车检测」：`目录设备 == 主账号机器码` 则生成独立码，否则用独立码 |
| 会不会仍撞主账号 | 不会。每个多开账号签到用**独立虚拟设备码**，主账号用**真实机器码**，二者不同 |
| 浏览器劫持还有用吗 | **未走通，已退役**。仅 `browser_takeover.rs` 作为应急预案留存，TRAE 改策略时才可能复用 |

---

## 1. 问题背景

TRAE 每天每个账号只能用一台设备签一次。多开时新客户端（2.3.78099）存在**机器级设备 ID** 强约束：

- 全机共用一个设备源（`%APPDATA%\TRAE\ahanet\tt_net_config.config`，如 `1234567890123456`），被主账号占用；
- 客户端启动时把机器级 `device_id` 强制同步到每个 data-dir，多开实例的 `icube-dc` 也被拉成机器码；
- 结果：主账号当天先签成功 → 其它账号再用同一机器码签 → 报 **9095 当前设备今日已签到** / **9074 设备维限额**。

### 为什么不能直接改文件（已实证全无效）

| 尝试 | 结果 |
| --- | --- |
| 改机器级全局源 / 各 data-dir 的 `tt_net_config.config` | 无效，客户端不读它 / 启动即覆盖 |
| 预注入 `storage.json` 的 `icube-dc` | 无效，客户端登录覆盖回机器级，且换 token 不读它 |
| 手工改写登录链接 | PKCE `code_verifier` 刷新失配，无法换 token |

> 根因：客户端换 token 上报的 DeviceID 来自**进程内存里已建好的登录 session**，磁盘怎么写都追不上——这也是早期劫持方案走不通的根本原因。

---

## 2. 当前方案（免劫持）——核心逻辑

隔离不改造客户端，而在**签到调用前选设备**。见 [trae_instance.rs](../src-tauri/src/trae_instance.rs) 的 `get_or_create_checkin_device_id`：

```rust
let dir_dev = read_instance_aha_device(dir);                  // 读目录现有 icube-dc 设备
let main_dev = crate::trae_auth::read_main_aha_device_id();   // 读主账号真实机器码

// 关键判断:目录设备是否撞了主账号机器码
let collides_main = match (&dir_dev, &main_dev) {
    (Some(d), Some(m)) => d == m,      // 目录设备 == 主账号机器码 → 撞车
    _ => false,
};
let chosen = if collides_main {
    generate_aha_device_id()           // 撞了 → 换全新的独立虚拟设备码
} else {
    dir_dev.unwrap_or_else(generate_aha_device_id)  // 目录自带独立码则沿用,否则生成
};
```

- **主账号**（无独立 data-dir）直接返回 `None`，签到沿用自身真实机器码。
- **多开账号**：若目录设备已独立 → 直接用它；若目录设备撞了主账号机器码 → **换独立码**；都没 → 现生成。
- 生成的独立码**持久化**为账号 `checkinDeviceId`，之后每天复用，不再变。
- 全程**不碰客户端 storage.json**，写入的是 TraeMate 自身账号数据，手动登录也不怕被重置。

### 隔离效果

| 账号 | 设备码类型 | 示例 | 来源 |
| --- | --- | --- | --- |
| 主账号 | 真实设备码 | `1234567890123456` | 机器级，主账号自己 |
| 多开 A | 虚拟设备码 | `2234567890123456` | 目录自带独立码 |
| 多开 B | 虚拟设备码 | `3234567890123456` | 撞机器码 → 自动换新独立码 |

三个码互不相同、均非主账号机器码 → 服务端判为三台设备 → 全部签到成功。

---

## 3. 涉及的代码/文件

| 文件 | 作用 |
| --- | --- |
| [`trae_instance.rs`](../src-tauri/src/trae_instance.rs) | `get_or_create_checkin_device_id`（撞车检测 + 独立码持久化）、`generate_aha_device_id`、`prepare_new_login_dir`（只写独立 machineid / tt_net_config，不注入 icube-dc） |
| [`checkin.rs`](../src-tauri/src/checkin.rs) | 签到用账号 `checkin_device_id` 作为 `x-device-id` 请求头 |
| [`trae_auth.rs`](../src-tauri/src/trae_auth.rs) | `read_main_aha_device_id` 读主账号真实机器码 |
| [`models.rs`](../src-tauri/src/models.rs) | `Account` 新增 `checkin_device_id` 字段 |
| [`AccountCard.vue`](../src/components/AccountCard.vue) | 展示真实设备码（主）/ 虚拟设备码（多开） |

---

## 4. 排障速查

| 现象 | 根因 | 对策 |
| --- | --- | --- |
| 9095 当前设备今日已签到 | 账号签到用的设备与当天已签主账号相同（机器码） | 走独立 `checkinDeviceId`（本方案自动处理） |
| 9074 | 用了未注册/不匹配的随机 device | 改用目录已用独立码或撞车换新码后持久化 |
| token 设备不匹配（20403/网络失败） | 授权设备 ≠ 换 token 设备（客户端 session 定死） | 属劫持时代问题；免劫持下正常机器码登录一次即可 |

---

## 5. 早期探索：浏览器劫持（未走通，退役）

浏览器劫持法曾试图「在客户端发起登录 URL 的瞬间拦截改写为独立设备」来根治隔离，**未走通**，已整体移除接线：

- `browser_takeover.rs` 中保存的要点（为日后参考）：
  - Windows `UserChoice→ProgId` 才是默认浏览器真正命令键，直接改 `http/https` 键无效；
  - 需 `is_capture_command` 剔除上次劫持残留的自引用；
  - URL 与 `redirect_url` 内层都要改 device（双层改写）；
  - 客户端换 token 用 session 内存设备，改写磁盘追不上 → 方案失效点。
- 现状：仅 [`browser_takeover.rs`](../src-tauri/src/browser_takeover.rs) 以 `#![allow(dead_code)]` **作为应急预案模块留存**，不再被调用。若日后 TRAE 更新账户/设备策略导致免劫持失效，可从这里接回扩展（含内存级方案），需先取消 dead_code 抑制并重新接线。

> 流程图 `browser-takeover-flow.svg` 描绘的是此未走通方案的旧流程，仅供参考结构，不反映当前实现。