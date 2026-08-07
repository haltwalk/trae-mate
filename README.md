# TraeMate

> Trae Work 每日签到与多开账号管理工具(Tauri 2 + Vue 3 + Rust)

## 简介

TraeMate 是一款 Trae Work 桌面客户端的辅助工具,提供:

- **多账号管理**:导入多个 Trae Work 桌面账号,凭证经 Windows DPAPI 加密存储于本机
- **每日签到**:一键或定时自动签到,签到后刷新总积分
- **多开实例**:为每个账号启动独立 `data-dir` 的免登录 TRAE 实例(独立机器码,互不干扰,共享插件目录省磁盘)
- **动态托盘菜单**:系统托盘右键菜单按账号分组,快捷聚焦各多开实例窗口
- **Neumorphism UI**:跟随系统明暗(默认夜间模式)+ Teal 强调色

## 技术栈

- 前端:Vue 3 + Pinia + Vite + TypeScript(Neumorphism / Soft UI 风格)
- 后端:Rust + Tauri 2
- 加密:Windows DPAPI(凭证存储)+ AES-128-CBC / HMAC-SHA512(TRAE auth 信封)

## 开发

```bash
pnpm install
pnpm tauri dev      # 开发模式
pnpm tauri build    # 打包成安装包
```

> 仅支持 Windows(TRAE Work 桌面客户端为 Windows 应用)。

## 借鉴项目

本项目的「多开实例」能力借鉴自以下开源项目,移植并适配了其中的加密信封、TRAE 路径扫描、进程启动/运行检测、机器码生成与窗口聚焦等逻辑:

- **[Trae-Work-CN-Account-Manager](https://github.com/xhrxgr/Trae-Work-CN-Account-Manager)** —— 多开 / 切换账号的核心实现(`encrypt_solo_cn_auth_info`、`machine.rs` 等)。其加密信封与本项目原有的 `decrypt_trae_auth_info` 同源(常量与 header 完全一致),故移植零风险。

签到逻辑为本项目原有实现,非借鉴。

## 声明

本项目仅供学习与个人使用。Trae Work 是字节跳动旗下产品,本项目与其无任何关联。使用本工具产生的任何后果(如账号风控、封禁等)由使用者自行承担。

## License

[MIT](./LICENSE)
