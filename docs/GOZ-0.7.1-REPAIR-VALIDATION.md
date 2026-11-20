# goz v0.7.1 硬链接修复：来源、验证与上线门禁

> 2026-11-20；**研究候选版，尚未部署生产**。本文件是本地审计记录，不是官方签名或可验证的第三方供应链证明。

## 1. 上游版本与源码

- 官方仓库：<https://github.com/mustafaahci/goz>。
- 当前官方发布：`v0.7.1`，`upstream/main` 与 tag 均指向 `93b7df05f58d73259d293d66d83d8abf23fde55f`。
- 本地研究基线 `D:\Workspace\goz-v071-hardlink-repair`，分支 `research/goz-v0.7.1-hardlink-repair`。
- 构建时源代码版本（部署脚本提交）：`40696129f0f7fe320b1ed55ab1b209c28984bfeb`。
- 变动范围：Win32 硬链接枚举句柄生命周期、USN 跟踪重试队列、索引完整性状态、协议/CLI状态、中文Windows ANSI测试及上游两处等价Clippy条件化简、受控部署脚本。
- 上游已有补丁 `5c584f4` 避免重复 Win32 枚举与重复失败计数，但不会为真正失败的 FRN 保留待重试状态；研究版的改动不能视为官方发布内容。

## 2. 编译环境与产物

- Rust/Cargo 1.94.0（`rustc` commit `4a4ef493e3a1488c6e321570238084b38948f6db`），`x86_64-pc-windows-msvc`。
- `Cargo.lock` SHA-256：`D347AA6C3999D4BE3E4A2C10F0A4389139137FCE1EF60579CBF03042D5157124`。
- 编译命令：`cargo +1.94.0 build --release -p goz-daemon -p goz-cli`。
- `target\release\gozd.exe`，5,024,256 字节，SHA-256：
  `1E36094B45EA80A79315593072736787A1946F3DC81F552719003B3CC4FD617A`。
- `target\release\goz.exe`，3,485,696 字节，SHA-256：
  `7DDB23E6EDA426AAF0FBF5BF7E5D9CFD9E54C1778852991155067F663B40CD94`。

这组数据允许将当前本机文件与本地源码记录对应起来，但**本地 Hash 和构建命令仍然不足以证明二进制来自可信的独立构建方**。如果管理员审批要求更高级别的证明，应使用可信CI出具的带签名构建溯源，不得借口这些本地信息绕过审批。

## 3. 已通过的验证

1. `cargo +1.94.0 test --workspace --quiet`：**713 项通过，无跳过**。
2. `cargo +1.94.0 fmt --all --check`：通过。
3. `cargo +1.94.0 clippy --workspace --all-targets -- -D warnings`：通过。
4. Windows 真实 NTFS：150 轮并发硬链接创建/删除，另一路线程按文件 ID 查询；最终稳定文件名准确，测试目录已清理。
5. 状态协议：`Live` 卷有当前未修复 FRN 时，`Status.volumes_incomplete`、`Hello.ready` 与每页查询的 `volumes_incomplete` 一致标记；恢复后历史计数保留、当前待修复数归零。
6. 索引恢复：同一 USN 批次的最终删除取消待办，父目录稍后才出现时重新核对成功，保留官方同批重复应用顺序。
7. 候选客户端 `goz.exe` 对已安装旧 daemon 的协议兼容：`--status` 可读取，`-n 8 AGENTS.md` 的前八条结果和旧客户端一致。

## 4. 当前阻塞

**管理员权限部署申请被拒绝**：`DENY — High impact EXE payload provenance is missing; fixed hashes alone do not establish trusted origin.`
不得重新包装、换名字或另走无审批提升权限路径绕过。当前生产 `C:\Program Files\goz` 下 EXE 保持原状，服务仍自动运行。

仍未完成的验收包括：

- 在受信任的来源证明满足审批要求并获批准后，才能部署到 SYSTEM 服务；
- 真实 D 盘累计失败到底由哪些 Win32 API、错误码和文件类型引起，还没有证据；
- 生产首次重建后及持续活动期间，待修复 FRN 是否自动归零、全路径检索是否始终完整；
- 长期运行下内存约 5 MiB 工作集的目标，暂未纳入此修复分支。

## 5. 受控部署方案

`scripts/deploy-goz-repair.ps1` 在不传参数时仅校验来源 Hash、当前 SCM 注册路径和备份目标；`-Apply` 需要独立管理员审批，将旧二进制备份到 `C:\Program Files\goz\rollback-20261120-v055`，然后在校验新文件 Hash 后替换并重启服务。服务和 C/D 卷恢复 `Live` 才算试点部署成功。失败时自动回滚；`-Rollback` 则另行请求授权进行显式回滚。

**切勿把脚本存在视为已获部署授权。** 当前状态是研究候选版，不是生产成功记录。
