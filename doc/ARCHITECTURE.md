# PicoHarness 设计文档(最终)

状态:定稿。**P1 已实现并通过实机验证(2026-10-15)**,分阶段进度见 §9。
本文是唯一设计文档;相关既有规格引用见文末「参考」。

## 1. 目标

双 Pico 载板:Debug 核 = 探针,Work 核 = 目标。单一 USB 连 PC(Work 不直连,不改板)。
把「一个私有协议承载一切」拆成标准接口实现功能分离,且代码命名统一。

## 2. 总体架构

```
PC ── USB(单口,复合)
   ├─ IF0  CMSIS-DAP v2 (0x04/0x85)           → SWD(OpenOCD/probe-rs/GDB)
   ├─ IF1/2 CDC ACM「PH Terminal」(0x02/0x83,notify 0x82)
   │         PH 协议消息(protobuf):control(原 PDBG)/console/配置/bulk 协调
   ├─ IF3  vendor「PH Terminal Bulk」(FF/50/01,0x05/0x86)
   │         数据面:大量数据批传输,载荷保持 raw;可经 SPI 转发 Work
   ├─ IF4/5 CDC ACM「Pico Harness Logic Analyzer」(0x06/0x87,notify 0x81)
   │         SUMP 协议 → PulseView/sigrok
   └──── Debug ⇄ Work: pico_harness(UART PH 帧 ≤128B + SPI ≤2048B,自适应,不变)
```

一句话:**host⇄Debug = PH 协议(CDC,protobuf)+ PH Terminal Bulk(数据面,raw);Debug⇄Work = pico_harness(UART/SPI)。**

- PH 协议消息是「消息化的原 PDBG」:PDBG 独立接口消失,电源/测量/RUN/BOOTSEL/GPIO/console 全部是消息。
- 大量数据只走 bulk(raw);编码只存在于 CDC 控制面,SPI/批量载荷不编码。

## 3. USB 接口

| 序号 | 角色 | 类/子类/协议 | 端点 | 说明 |
|---|---|---|---|---|
| 0 | SWD | FF/00/00 | 0x04 / 0x85 | 不变 |
| 1,2 | PH Terminal(协议通道) | CDC ACM | 0x02 / 0x83(+0x82 notify) | 全部 PH 消息 |
| 3 | PH Terminal Bulk(数据面) | FF/50/01 | 0x05 / 0x86 | raw 载荷 |
| 4,5 | Logic Analyzer | CDC ACM | 0x06 / 0x87(+0x81 notify) | SUMP |

VID/PID `0x2e8a/0x000c`、产品名、`usb_serial` 不变;MS-OS20 覆盖 IF0(可选扩展 IF3)。
端点号内不冲突(共 10/16)。

## 4. PH 协议(protobuf)

- **工具链**:host 用 **prost**(Rust,编译期 codegen);固件用 **nanopb**(C,静态缓冲、免分配)。
  共享 schema:`proto/ph.proto`(包 `picoharness.v1`),两端从同一文件生成。
- **信封/成帧**:长度分隔 `<varint length><PhMessage bytes>`(prost/nanopb 的 delimited
  encode/decode)。USB 链路自带 CRC/重传,不再另加应用层校验;序列号 `seq` 做应答匹配。
- **消息结构**:

```proto
message PhMessage {
  uint32 seq = 1;          // 请求/应答 echo;单向事件用 0
  uint32 status = 2;       // 0=ok 1=frame 2=cmd 3=arg 4=busy 5=crc 6=route
  oneof body {
    // control(原 PDBG)
    GetInfo get_info = 16;  GetStatus get_status = 17;
    PowerSet power_set = 18; ResetPulse reset_pulse = 19;
    BootselSet bootsel_set = 20; EnterBootsel enter_bootsel = 21;
    Measure measure = 22;   GpioRead gpio_read = 23;
    GpioDiag gpio_diag = 24; DebugBootsel debug_bootsel = 25;
    // terminal
    ConsoleTx console_tx = 32;    bytes data       // ≤128B,host→Debug→Work
    ConsoleData console_data = 33; bytes data      // Debug→host(短)
    TermConfig term_config = 34;  {baud,data_bits,stop_bits,parity}
    TermStatus term_status = 35;
    // bulk 协调(载荷在 IF3)
    BulkTxBegin bulk_tx_begin = 40; {route,len,crc32c}
    BulkTxDone  bulk_tx_done  = 41; {ok,crc_recv}
    BulkRxAvail bulk_rx_avail = 42; {len}
    BulkRxBegin bulk_rx_begin = 43;
    BulkRxDone  bulk_rx_done  = 44; {ok}
  }
}
```

- 字段编号扁平分配,不存在原 16 进制命令编号冲突问题。
- `TermConfig` → Debug 对 Work 执行 pico_harness 波特率事务(0x03/0x04/0x05 + 回滚);CDC line-coding 不使用。

## 5. 数据面(PH Terminal Bulk,raw)

- IF3 载荷是**裸字节流**,不做帧/编码;长度与完整性由 `BulkTxBegin.len` 与 `BulkTxDone.crc32c` 界定。
- `route`:`WORK_SPI=1`(首版,载荷由 Debug 按 ≤2048B 块 pico_harness 转发到 Work);`DEBUG=0` 预留。
- 流程(host→Work):`BulkTxBegin(CDC)` → 载荷(IF3,raw)→ `BulkTxDone(CDC,CRC 校验)`。
  Work→host:`BulkRxAvail` → `BulkRxBegin` → 载荷(IF3)→ `BulkRxDone`。
- 单并发:一个数据面长事务进行期间,其它 bulk 协调消息返回 `busy`;控制消息可穿插。

## 6. 其它接口

- **SWD**:CMSIS-DAP v2,不变。
- **LA**:CDC + SUMP(libsigrok `ols` 驱动),`0x02` ID 应答 `'SLA'+1`、divider/count/flags/trigger、32-bit 样本;
  16 路逻辑通道序固定(见 `probe-logic-analyzer.md`),pre-trigger 为回读流前 25% 历史。PulseView 直用。

## 7. Debug⇄Work

`doc/protocol.md` 的 pico_harness:**不变**。固件承担模式发现(Hello)、console `0x02`、
批量 `0x06/0x08/0x87` 与波特率事务——即 Debug⇄Work 的自适应编排在固件侧。

## 8. 代码组织与命名

- **`host/ph-bridge`**(crate `ph-bridge`,lib `ph_bridge`)是 PC⇄Debug 的连接库:
  协议/控制/console/bulk 的 host 侧实现(prost),并提供 Rust + Python 双接口
  (`feature="python"`,PyO3→`phbridge`)。desktop、`ph` CLI、host 脚本统一经它;
  `desktop/device_control` 的接入/吸收在 P4。
- 命名规范:`ph_<子前缀>_<名>`。

| 子前缀 | 组件 |
| --- | --- |
| `ph_bridge_` | `host/ph-bridge`(Rust crate) |
| `ph_py_` | Python 绑定(PyO3 层) |
| `ph_cli_` | `ph` 命令行 |
| `ph_app_` | desktop GUI(`picoharness-desktop`) |
| `ph_fw_` | 固件 `firmware/harness-probe`(下接模块:`_msg_`协议/`_terminal_`/`_bulk_`/`_la_`/`_usb_`/`_target_`) |

- 例外(不上 `ph_`):wire magic、PID/VID、接口字符串(「PH Terminal」等)、产品名、第三方代码、
  Debug⇄Work 的 `doc/protocol.md` 字段名。

## 9. 实现进度与验证

P1 已完成并实机验证(RP2040 Debug core,`/dev/ttyACM*` + pyusb 实测):

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| P1 固件 | USB 拓扑(IF0 DAP / IF1-2 CDC PH Terminal / IF3 vendor FF/50/01 / IF4-5 CDC LA)、PH 协议 protobuf(nanopb)、control+terminal 消息、bulk `DEBUG` 路由(载荷接收 + CRC) | ✅ 完成,实机 13 项通过 |
| P1 host | `host/ph-bridge`(prost + framing + `PhClient`)、`host/phbridge`(python)、`host/tests` 离线+实机测试、`build.sh test` 接入 | ✅ 完成,`./build.sh test --offline` 绿 |
| P2 控制台桥 | console 的 pico_harness 模式(Hello 发现、`0x02` 帧化、`0x90` 事件)+ 波特率事务(0x03-0x05,prepare/verify/commit+回滚)+ `ph` CLI;bulk `WORK_SPI` 转发归 P3 | ✅ 实机验证:REPL 交互 + 波特率往返(460800/115200/230400) |
| P3 bulk | `WORK_SPI` 转发(host→Work,块式 0x06/0x86/SPI/0x87 + 整体 CRC-32C)+ `ph bulk` CLI;Work→host 读回 | 🔶 host→Work ✅ 实机(100B/5000B 多块);读回 ⬜ |
| P3 LA | CDC2 SUMP(libsigrok `ols`,ID `1ALS`)、PulseView 验证 | 🔶 设备侧协议原生验证完美(10kHz@1Msps);sigrok `--scan` 识别成功;驱动 divider 语义(100MHz 参考)与重复采集稳定性待精调 |
| P4 host | `host/pdbg` 退役、`ph` CLI(python)、PyO3 绑定(feature `python` + maturin) | ✅ `./build.sh test --offline` 绿 |
| P4 desktop | `device_control::PdbgLink` 改为 `ph_bridge` 适配层(控制面走 PH;UART/SPI/capture 通道已改道并明确报错) | ✅ workspace 编译零警告、73 测试过、实机 `live` 集成测试过 |
| P5 改名 | 固件自有模块统一 `ph_fw_*`(worker/target/swd/util/usb/la)+ 宏前缀 `PDBG_`→`PH_FW_`,文件随模块重命名 | ✅ 双板编译、离线门闸、实机 7 项设备测试全过 |
| P5 改名(host/desktop) | desktop crate `ph-app` / `ph_app_link`、类型 `PhBoardLink`、固件标识串校验同步 | ✅ 编译零警告、73 测试 + 实机 `live` 集成测试过 |
| P5 改名(宏/线程) | 固件自有局部宏与线程入口 `PH_FW_*`/`ph_fw_*`(CMSIS-DAP 契约宏与 `tud_*`/`DAP_*` 除外) | ✅ 双板编译、离线门闸、实机 7 项 + sigrok 扫描过 |

阶段边界按里程碑提交;每阶段`./build.sh test --offline` + 实机冒烟为门。

## 10. 待办(后续阶段前敲定)

- Windows 串口归属/驱动范围(MAC/Linux 优先);
- SUMP 客户端内置(ph_bridge)vs 封装 sigrok-cli(P3 前定)。

## 参考

`doc/protocol.md`(Debug⇄Work 线协议)、`doc/work-library.md`、`doc/probe-logic-analyzer.md`、`doc/probe-firmware.md`、`doc/getting-started.md`。