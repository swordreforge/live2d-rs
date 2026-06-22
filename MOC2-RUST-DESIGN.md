# MOC2 Rust 实现设计文档

## 一、概述

本文档描述从 Python Cubism 2.1 实现（`live2d-v2-main`）移植到 Rust 的 MOC2 二进制解析器和运行时插值系统。目标是绕过 Cubism 5 Core 原生库，直接加载 `.moc` 文件。

**参考代码：** `/home/swordreforge/Downloads/live2d-v2-main/live2d/core/`

---

## 二、MOC 二进制格式

### 格式特征

- **字节序：** 大端序（Big-endian）
- **魔数：** `moc`（3 字节: `0x6D 0x6F 0x63`），第 4 字节为版本号
- **支持版本：** 8~11（v2.8 ~ v2.11）
  - `LIVE2D_FORMAT_VERSION_V2_8_TEX_OPTION = 8`
  - `LIVE2D_FORMAT_VERSION_V2_10_SDK2 = 10`
  - `LIVE2D_FORMAT_VERSION_V2_11_SDK2_1 = 11`
  - `LIVE2D_FORMAT_VERSION_AVAILABLE = 11`
- **序列化核心：** 类型标签 + 对象引用表

### 类型标签系统

`Live2DObjectFactory` 映射表：

| 标签 | 类型 | 说明 | 是否是 `Id` |
|------|------|------|-------------|
| 0 | `null` | 空值 | 否 |
| 1 | `String` | UTF-8 字符串 | 否 |
| 15 | `Array<T>` | 对象数组 | 否 |
| 16, 25 | `Int32Array` | int32 数组 | 否 |
| 26 | `Float64Array` | float64 数组 | 否 |
| 27 | `Float32Array` | float32 数组 | 否 |
| 33 | `ObjectRef` | 回指之前读过的对象（按 index） | 否 |
| 48~ | `KnownType` | 通过 Factory 创建的类型 | 否 |
| 50, 51, 60, 134 | `Id` | 字符串 ID 包装 | **是** |
| 65 | `WarpDeformer` | 网格变形器 | 否 |
| 66 | `PivotManager` | 参数→值映射管理器 | 否 |
| 67 | `ParamPivots` | 单个参数的枢轴数据（控制一个维度） | 否 |
| 68 | `RotationDeformer` | 旋转变形器（仿射变换） | 否 |
| 69 | `AffineEnt` | 单个仿射变换关键帧 | 否 |
| 70 | `Mesh` | 可绘制网格 | 否 |
| 131 | `ParamDefFloat` | 参数定义 | 否 |
| 133 | `PartsData` | 部件 | 否 |
| 136 | `ModelImpl` | 根对象 | 否 |
| 137 | `ParamDefSet` | 参数定义集合 | 否 |
| 142 | `Avatar` | 子部件/组件 | 否 |

### 对象引用机制

序列化流维护一个全局对象引用表（`objects[]`）：

```
遇到新对象 → readKnownTypeObject(type_tag) → 序列化 → push to objects[]
遇到 OBJECT_REF(33) → readInt32() → 返回 objects[index]
```

这使得同一对象可在多处引用（例如，同一个 `PivotManager` 被多个 `ParamPivots` 共享，同一个 `Id` 被多处引用）。

### 变长整数编码

`readNumber()` 使用 VLQ 风格编码（最多 4 字节）：

```
字节 1: bit7=0 → 7 位值 (0-127)
字节 1: bit7=1 + 字节 2: bit7=0 → 14 位值
字节 1: bit7=1 + 字节 2: bit7=1 + 字节 3: bit7=0 → 21 位值
字节 1: bit7=1 + 字节 2: bit7=1 + 字节 3: bit7=1 + 字节 4 → 28 位值
```

### 根序列化结构

```
ModelImpl (类型标签 136):
  1. paramDefSet:   ObjectRef → ParamDefSet (137) | ObjectRef
  2. partsDataList: ObjectRef → Array<PartsData> (15)
  3. canvasWidth:   int32 (直接值)
  4. canvasHeight:  int32 (直接值)

ParamDefSet (类型标签 137):
  1. paramDefList: ObjectRef → Array<ParamDefFloat> (15)

ParamDefFloat (类型标签 131):
  1. minValue:     float32 (直接值)
  2. maxValue:     float32 (直接值)
  3. defaultValue: float32 (直接值)
  4. paramId:      ObjectRef → Id (50/51/60/134)

PartsData (类型标签 133):
  1. locked:       bit    (通过 readBit)
  2. visible:      bit
  3. id:           ObjectRef → Id
  4. deformerList:  ObjectRef → Array<Deformer> (15)
  5. drawDataList:  ObjectRef → Array<Mesh> (15)

WarpDeformer (类型标签 65):
  1. id:            ObjectRef → Id
  2. targetId:      ObjectRef → Id
  3. col:           int32
  4. row:           int32
  5. pivotMgr:      ObjectRef → PivotManager (66)
  6. pivotPoints:   ObjectRef → Float32Array (27)
  7. pivotOpacities: Float32Array (27)  [v2.10+]

RotationDeformer (类型标签 68):
  1. id:            ObjectRef → Id
  2. targetId:      ObjectRef → Id
  3. pivotMgr:      ObjectRef → PivotManager (66)
  4. affines:       ObjectRef → Array<AffineEnt> (15)
  5. pivotOpacities: Float32Array (27)  [v2.10+]

AffineEnt (类型标签 69):
  1. originX:      float32
  2. originY:      float32
  3. scaleX:       float32
  4. scaleY:       float32
  5. rotationDeg:  float32
  6. reflectX:     boolean [v2.10+]
  7. reflectY:     boolean [v2.10+]

PivotManager (类型标签 66):
  1. paramPivotTable: ObjectRef → Array<ParamPivots> (15)

ParamPivots (类型标签 67):
  1. paramId:      ObjectRef → Id
  2. pivotCount:   int32
  3. pivotValues:  ObjectRef → Float32Array (27)

Mesh (类型标签 70):
  [IDrawData 基类:]
  1. id:              ObjectRef → Id
  2. targetId:        ObjectRef → Id
  3. pivotMgr:        ObjectRef → PivotManager (66)
  4. averageDrawOrder: int32
  5. pivotDrawOrders:  ObjectRef → Int32Array (16/25)
  6. pivotOpacities:  ObjectRef → Float32Array (27)
  7. clipID:          ObjectRef → Id  [v2.11+]
  [Mesh 扩展:]
  8. textureNo:       int32
  9. pointCount:      int32
  10. polygonCount:   int32
  11. indexArray:     ObjectRef → Int16Array (从 readObject 读, 类型 16/25)
  12. pivotPoints:    ObjectRef → Float32Array (27)
  13. uvs:            ObjectRef → Float32Array (27)
  14. optionFlag:     int32  [v2.8+]
      - 如果 optionFlag & (1<<5) != 0: 读取并跳过 int32（未处理字段）
      - colorCompositionType = (optionFlag & 0x1E) >> 1
        - 0 = normal
        - 1 = screen
        - 2 = multiply
      - 如果 optionFlag & 32 != 0: culling = false

Avatar (类型标签 142):
  1. id:             ObjectRef → Id
  2. drawDataList:   ObjectRef → Array<Mesh> (15)
  3. deformerList:   ObjectRef → Array<Deformer> (15)
```

### 校验

版本 >= 8 时，MOC 文件末尾有额外的校验字节：
```
fileEndCheck1: u16 = -30584 (0x8888)
fileEndCheck2: u16 = -30584 (0x8888)
```

### 格式版本差异总结

| 版本 | 新增内容 |
|------|---------|
| v2.8 | `Mesh.optionFlag`, 文件末尾校验 |
| v2.10 | `AffineEnt.reflectX/Y`, `Deformer.pivotOpacities` |
| v2.11 | `IDrawData.clipID`（剪裁蒙版关联） |

---

## 三、反向依赖树（静态结构）

```
ModelImpl
├── ParamDefSet
│   └── Array<ParamDefFloat>
│       └── ParamDefFloat { min, max, default, id: Id }
│
└── Array<PartsData>
    └── PartsData { visible, locked, id: Id }
        ├── Array<Deformer>
        │   ├── WarpDeformer { id, targetId, col, row, pivotMgr, pivotPoints, pivotOpacities }
        │   └── RotationDeformer { id, targetId, pivotMgr, affines: Array<AffineEnt>, pivotOpacities }
        └── Array<Mesh>
            └── Mesh { id, targetId, pivotMgr, orders, opacities, clipID,
                       textureNo, vertices, indices, uvs, colorComposition, culling }
```

**关键说明：**
- `Mesh.targetId` 指向该网格绑定的 `Deformer.id`（或其父变形器链）
- `Deformer.targetId` 指向父 `Deformer.id` 或 `DST_BASE`（根）
- 一个 `PivotManager` 可能被多个 `Mesh`/`Deformer` 共享
- Deformer 的拓扑排序：子永远在父之前，`DST_BASE` 是根目标

---

## 四、运行时插值管线

### 4.1 总体流程

每帧调用一次 `Moc2Model::update()`：

```
1. 检测参数变化
   └── 对比当前值和上一帧值，标记 updatedParamFlags

2. 处理所有 Deformer（按拓扑顺序）
   ├── 2a. setupInterpolate: 检查绑定参数是否变化
   │   ├── PivotManager.calcPivotValues()
   │   │   └── 对每个 ParamPivot，根据当前参数值计算 (pivotIndex, t)
   │   ├── PivotManager.calcPivotIndices()
   │   │   └── 生成张量积索引表和插值 t 值数组
   │   └── 根据 Deformer 类型执行具体插值
   │
   └── 2b. setupTransform: 递归应用父变形器
       ├── 如果无父变形器: totalOpacity = interpolatedOpacity
       ├── 如果有父变形器: 父变形器 transformPoints 当前控制点/仿射参数
       └── totalOpacity *= parent.totalOpacity

3. 处理所有 Mesh（DrawData）
   ├── 3a. setupInterpolate
   │   ├── 插值顶点位置 (interpolatePoints)
   │   ├── 插值绘制顺序 (interpolateDrawOrder)
   │   └── 插值透明度 (interpolateOpacity)
   │
   └── 3b. setupTransform
       └── 通过绑定的变形器链变换顶点位置

4. 构建绘制顺序链表
   ├── 按 interpolatedDrawOrder 排序
   └── 构建 orderList_firstDrawIndex / orderList_lastDrawIndex / nextList_drawIndex
```

### 4.2 ParamPivots 插值原理

`ParamPivots` 将一个参数值映射到 `pivotIndex` 和 `t ∈ [0, 1]`：

```
给定参数值 value, pivotValues = [v0, v1, v2, ..., vn-1]

if value < v0 - GOSA:
    pivotIndex = 0, t = 0, outside = true
elif value in [v0-GOSA, v0+GOSA]:
    pivotIndex = 0, t = 0
else:
    for i in 1..n:
        if value < v_i + GOSA:
            if value > v_i - GOSA:  // 精确在枢轴上
                pivotIndex = i, t = 0
            else:                    // 在 v_{i-1} 和 v_i 之间
                pivotIndex = i-1, t = (value - v_{i-1}) / (v_i - v_{i-1})
            break
    if not found:  // value > v_{n-1}
        pivotIndex = n-1, t = 0, outside = true

GOSA = 0.0001
```

### 4.3 PivotManager 多维插值

多个 `ParamPivots` 组成多维控制空间。`calcPivotIndices` 生成张量积索引：

```
给定 k 个 ParamPivots，每个有 pivotCount_i 个枢轴值：
- 总枢轴组合 = product(pivotCount_i)
- 需要 dim = ceil(log2(总组合)) 个二分子空间
- 结果：
  - indices[]: 长度为 2^dim 的索引数组（指向 AffineEnt/Mesh 数组）
  - t_values[]: dim 个插值值
```

**具体算法（Python 的 `calcPivotIndices`）:**

```
step = 1
for each paramPivot:
    idx = paramPivot.tmpPivotIndex
    t = paramPivot.tmpT
    
    if t == 0:  // 精确在枢轴上
        for each position in indices[]:
            indices[position] += idx * step
    else:        // 在枢轴之间
        for each position in indices[]:
            if (position / half) % 2 == 0:
                indices[position] += idx * step
            else:
                indices[position] += (idx + 1) * step
        t_values.insert(t)
        half *= 2
    
    step *= paramPivot.pivotCount

indices[last] = 65535 (哨兵)
t_values[last] = -1 (哨兵)
```

### 4.4 WarpDeformer 网格插值

控制点网格 `(row+1) × (col+1)` 个点，每个点 2 个坐标：

```
输入：pivot_points = [x0,y0, x1,y1, ..., x_{n-1},y_{n-1}]
     其中 n = (row+1) * (col+1)

网格顶点从 pivotPoints 插值得到：
  PivotManager 驱动 → PivotManager.calcPivotValues()
  → PivotManager.calcPivotIndices()
  → 在网格顶点间做双线性插值

最终顶点变换：
  src_uv = 顶点 UV 坐标（归一化到 [0,1]）
  grid_pos = 从 (row+1)×(col+1) 网格中双线性插值得到的变换后控制点
  
  行索引 r = src_uv.x * row
  列索引 c = src_uv.y * col
  
  if 0 <= r < row and 0 <= c < col:
    // 在网格内：标准双线性插值
    dst = lerp(lerp(grid[r,c], grid[r+1,c], frac(r)),
               lerp(grid[r,c+1], grid[r+1,c+1], frac(r)),
               frac(c))
  else:
    // 边界外推：使用边缘外延
    // (详见 Python 的 WarpDeformer.transformPoints_sdk2)
```

### 4.5 RotationDeformer 仿射插值

在多个 `AffineEnt` 之间做 N 维线性插值：

```
给定 indices[] 和 t_values[], dim:
  if dim == 0:
    interpolated = affines[indices[0]]
  elif dim == 1:
    a0 = affines[indices[0]]
    a1 = affines[indices[1]]
    t = t_values[0]
    interpolated = lerp(a0, a1, t)
  elif dim == 2:
    a00, a01, a10, a11 = affines[indices[0..3]]
    t0, t1 = t_values[0], t_values[1]
    row0 = lerp(a00, a01, t0)
    row1 = lerp(a10, a11, t0)
    interpolated = lerp(row0, row1, t1)
  ...
  
  反射标志 (reflectX/Y) 从 affines[indices[0]] 取（不插值）

AffineEnt 字段：
  originX, originY       // 变换原点（平移）
  scaleX, scaleY         // 缩放
  rotationDeg            // 旋转角度（度）
  reflectX, reflectY     // 镜像翻转
```

**变换单个点：**
```
// rotation = rotationDeg * PI / 180
cos_r = cos(rotation)
sin_r = sin(rotation)

sx = -1 if reflectX else 1
sy = -1 if reflectY else 1

dst_x = cos_r * scale * sx * src_x + (-sin_r) * scale * sy * src_y + origin
dst_y = sin_r * scale * sx * src_x + cos_r   * scale * sy * src_y + origin
```

### 4.6 透明度链

```
沿变形器链从根到叶累积：

totalOpacity = 1.0
for each deformer in chain(root → leaf):
    totalOpacity *= deformer.interpolatedOpacity

Mesh 最终透明度：
  meshOpacity = mesh.interpolatedOpacity * partsOpacity * baseOpacity
  其中 baseOpacity = 绑定变形器链的 totalOpacity
```

### 4.7 绘制顺序插值

每个 `Mesh` 有 `pivotDrawOrders`（与参数关联的绘制顺序值数组），通过相同的 `PivotManager` 做插值：

```
interpolatedDrawOrder = interpolateInt(PivotManager, pivotDrawOrders)
// 使用 PivotManager 的 calcPivotValues + calcPivotIndices
// 对整数值做张量积插值（四舍五入到最近整数）
```

---

## 五、Rust 数据结构设计

### 5.1 静态数据（从 MOC 二进制解析）

```rust
pub(crate) struct Moc2Data {
    pub canvas_width: i32,
    pub canvas_height: i32,
    pub param_defs: Vec<ParamDef>,
    pub parts: Vec<Part>,
    pub deformers: Vec<DeformerNode>,  // 拓扑排序后的所有变形器
    pub drawables: Vec<DrawableData>,
}

pub struct ParamDef {
    pub id: Id,
    pub min_value: f32,
    pub max_value: f32,
    pub default_value: f32,
}

pub struct Part {
    pub id: Id,
    pub visible: bool,
    pub locked: bool,
    pub deformer_indices: Vec<usize>,   // 索引到 deformers[]
    pub drawable_indices: Vec<usize>,   // 索引到 drawables[]
}

pub struct DeformerNode {
    pub id: Id,
    pub target_id: Id,
    pub kind: DeformerKind,
    pub pivot_manager_offset: usize,      // 索引到 pivot_managers[]
    pub opacity_pivots: Vec<f32>,         // pivotOpacities
}

pub enum DeformerKind {
    Warp {
        col: i32,
        row: i32,
        pivot_points: Vec<f32>,           // (row+1)*(col+1)*2
    },
    Rotation {
        affines: Vec<AffineEnt>,
    },
}

pub struct AffineEnt {
    pub origin_x: f32,
    pub origin_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_deg: f32,
    pub reflect_x: bool,
    pub reflect_y: bool,
}

pub struct DrawableData {
    pub id: Id,
    pub target_deformer: Option<usize>,      // 索引到 deformers[]
    pub pivot_manager_offset: usize,          // 索引到 pivot_managers[]
    pub average_draw_order: i32,
    pub pivot_draw_orders: Vec<i32>,
    pub pivot_opacities: Vec<f32>,
    pub clip_id_list: Vec<Id>,
    pub texture_no: i32,
    pub vertex_count: i32,
    pub indices: Vec<u16>,
    pub pivot_points: Vec<f32>,              // vertex_count * 2
    pub uvs: Vec<f32>,                        // vertex_count * 2
    pub color_composition: ColorComposition,
    pub culling: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ColorComposition {
    Normal,  // 0
    Screen,  // 1
    Multiply,// 2
}

/// PivotManager 和 ParamPivot 由 `PivotManagerSet` 统一管理
pub(crate) struct PivotManagerSet {
    pub managers: Vec<PivotManager>,
    pub pivots: Vec<ParamPivot>,
}

pub(crate) struct PivotManager {
    pub pivot_indices: Vec<usize>,  // 索引到 pivots[]
}

pub(crate) struct ParamPivot {
    pub param_index: i32,           // 绑定到哪个参数（-2 = 未初始化）
    pub pivot_count: i32,
    pub pivot_values: Vec<f32>,
}
```

### 5.2 运行时状态

```rust
pub struct Moc2Model {
    data: Arc<Moc2Data>,
    params: Moc2Params,
    // 运行时缓存
    deformer_states: Vec<DeformerState>,
    drawable_states: Vec<DrawableState>,
    part_opacities: Vec<f32>,
    pivot_caches: Vec<ParamPivotCache>,
    // 绘制顺序链表
    order_first: Vec<i32>,
    order_last: Vec<i32>,
    next_draw: Vec<i32>,
    // 每帧重建
    render_orders: Vec<i32>,
}

pub struct Moc2Params {
    pub values: Vec<f32>,
    pub prev_values: Vec<f32>,
    pub updated: Vec<bool>,
}

struct DeformerState {
    // 插值结果
    interpolated_opacity: f32,
    total_opacity: f32,
    total_scale: f32,
    available: bool,
    // Warp 特有
    interpolated_points: Vec<f32>,
    transformed_points: Vec<f32>,
    // Rotation 特有
    interpolated_affine: AffineEnt,
    transformed_affine: Option<AffineEnt>,
}

struct DrawableState {
    parts_opacity: f32,
    base_opacity: f32,
    interpolated_opacity: f32,
    draw_order: i32,
    available: bool,
    param_outside: bool,
    interpolated_vertices: Vec<f32>,
    transformed_vertices: Vec<f32>,
}

struct ParamPivotCache {
    param_index: i32,
    tmp_pivot_index: i32,
    tmp_t: f32,
    init_version: i32,
}
```

---

## 六、API 设计

### 6.1 加载接口

```rust
impl Moc2Model {
    /// 从 MOC 二进制数据解析并构建运行时模型
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Moc2Error>;
}
```

### 6.2 每帧接口

```rust
impl Moc2Model {
    /// 更新模型：参数变化 → 变形器插值 → 顶点变换 → 排序
    pub fn update(&mut self);

    /// 设置参数值
    pub fn set_param_value(&mut self, index: usize, value: f32);
    pub fn set_param_value_by_id(&mut self, id: &str, value: f32);

    /// 读取参数值
    pub fn param_values(&self) -> &[f32];
    pub fn param_values_mut(&mut self) -> &mut [f32];
    pub fn param_ids(&self) -> Vec<&str>;
    pub fn param_mins(&self) -> Vec<f32>;
    pub fn param_maxs(&self) -> Vec<f32>;
    pub fn param_defaults(&self) -> Vec<f32>;

    /// 读取部件透明度
    pub fn part_opacities(&self) -> &[f32];
    pub fn part_opacities_mut(&mut self) -> &mut [f32];

    /// 读取绘制数据（每帧更新后调用）
    pub fn drawable_count(&self) -> usize;
    pub fn drawable_ids(&self) -> Vec<&str>;
    pub fn drawable_texture_index(&self, idx: usize) -> i32;
    pub fn drawable_vertex_count(&self, idx: usize) -> i32;
    pub fn drawable_vertices(&self, idx: usize) -> &[f32];
    pub fn drawable_uvs(&self, idx: usize) -> &[f32];
    pub fn drawable_indices(&self, idx: usize) -> &[u16];
    pub fn drawable_opacity(&self, idx: usize) -> f32;
    pub fn drawable_color_composition(&self, idx: usize) -> ColorComposition;
    pub fn drawable_culling(&self, idx: usize) -> bool;
    pub fn drawable_multiply_color(&self, idx: usize) -> [f32; 4];
    pub fn drawable_screen_color(&self, idx: usize) -> [f32; 4];
    pub fn drawable_masks(&self, idx: usize) -> &[usize];
    pub fn drawable_parent_part_index(&self, idx: usize) -> i32;

    /// 渲染顺序
    pub fn render_orders(&self) -> &[i32];

    /// Canvas 信息
    pub fn canvas_width(&self) -> i32;
    pub fn canvas_height(&self) -> i32;
}
```

---

## 七、BinaryReader Rust 实现设计

```rust
pub(crate) struct BinaryReader<'a> {
    buf: &'a [u8],
    offset: usize,
    format_version: u8,
    objects: Vec<MocObject>,
    bit_offset: u8,
    bit_byte: u8,
}

// 对象引用表的条目
enum MocObject {
    None,
    Id(Id),
    String(String),
    Int32Array(Vec<i32>),
    Float32Array(Vec<f32>),
    Deformer(DeformerRaw),
    PivotManager(PivotManagerRaw),
    // ... etc
    Untyped,  // 占位，类型已知但暂不解析
}

// 读取中间结果（factory 创建阶段）
enum MocRawObject {
    ModelImpl { param_def_set: usize, parts_data_list: usize,
                canvas_width: i32, canvas_height: i32 },
    PartsData { locked: bool, visible: bool, id: usize,
                deformer_list: usize, draw_data_list: usize },
    // ...
}
```

**读取策略：**
1. 先读取所有字节到 `objects[]` 引用表中（关联 `MocObject`）
2. 第一遍扫描后，类型确认的 `ObjectRef` 直接返回
3. 解析完成后将 `MocRawObject` 转换为 Rust 类型

---

## 八、集成方案

### 8.1 模块位置

```
live2d-core/src/
├── lib.rs          # 导出 pub mod moc2
├── moc2/
│   ├── mod.rs      # 公开 API 导出
│   ├── reader.rs   # BinaryReader + MOC 二进制解析
│   ├── types.rs    # Moc2Data, Part, DrawableData, Deformer 等类型
│   ├── pivot.rs    # PivotManager 插值计算
│   ├── deformer.rs # Warp/Rotation 变形器插值和变换
│   └── runtime.rs  # Moc2Model（完整运行时）
```

### 8.2 model_loader.rs 修改

```rust
#[derive(PartialEq)]
pub enum ModelFormat {
    Moc3,
    Moc2,
}

pub fn detect_model_format(dir: &Path) -> Result<ModelFormat> {
    // 优先检测 .model3.json → MOC3
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains(".model3.json") { return Ok(ModelFormat::Moc3); }
            }
        }
    }
    // 检测 .model.json + .moc → MOC2
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_file() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(".moc") { return Ok(ModelFormat::Moc2); }
            }
        }
    }
    Err(anyhow::anyhow!("No model files found"))
}
```

### 8.3 app.rs 修改——统一模型枚举

```rust
pub enum LoadedModel {
    Core(live2d_core::Model<'static>),
    Moc2(live2d_core::moc2::runtime::Moc2Model),
}
```

`AppState` 中的 `current_model` 从 `Option<Model<'static>>` 改为泛型化的访问层。

### 8.4 渲染器适配

在 renderer 中创建一个统一的数据提取层：

```rust
pub(crate) fn collect_drawables(
    model: &LoadedModel,
) -> Vec<DrawableRenderData> {
    match model {
        LoadedModel::Core(core) => collect_core_drawables(core),
        LoadedModel::Moc2(moc2) => collect_moc2_drawables(moc2),
    }
}
```

---

## 九、实现路线图

### Phase 1 — Binary Reader + Types

- 实现 `BinaryReader`（变长整数、类型标签、对象引用表）
- 定义所有 MOC2 类型的 Rust 结构体
- 实现 `read()` 方法反序列化完整 MOC 文件
- 验证：解析 `test-data/model.moc` 并打印结构树

### Phase 2 — Pivot Interpolation

- 实现 `PivotManager::calc_pivot_values()`
- 实现 `PivotManager::calc_pivot_indices()`
- 验证：给定参数值，正确计算枢轴索引和插值 t

### Phase 3 — Deformer + Drawable Interpolation

- 实现 `WarpDeformer` 网格插值 + 顶点变换
- 实现 `RotationDeformer` 仿射插值 + 顶点变换
- 实现变形器链递归变换
- 实现绘制顺序/透明度插值
- 验证：对比 Python 运行时的顶点位置输出

### Phase 4 — Viewer Integration

- `model_loader.rs`: MOC2 检测 + 加载
- `app.rs`: `LoadedModel` 枚举
- `renderer/mod.rs`: 统一 DrawableRenderData 提取
- 端到端验证：渲染 `wanko.moc` / `tsumiki.moc`

### Phase 5 — 扩展（可选）

- `.mtn` 动作解析器
- 物理引擎适配（`physics3.json`→ 已存在，`.physics` 格式需要解析器）
- 全部 test-data 模型验证

---

## 十、与 Python 实现的关键差异

| 方面 | Python | Rust |
|------|--------|------|
| 对象引用表 | 全局 `objects` list（纯动态） | `Vec<MocObject>` 枚举 + 第二阶段类型转换 |
| 变长整数 | `readNumber()` 逐个字节解析 | 相同算法 |
| 数值容器 | Python list / numpy | `Vec<f32>`, `Vec<i32>`, `Vec<u16>` |
| 变形器排序 | 拓扑排序（每次 init） | 初始化时排序一次，缓存 |
| 内存管理 | 引用计数 | `Arc<Moc2Data>` 共享静态数据 + 运行时独有 |
| 可选值 | `None` | `Option<T>` |
| Id 系统 | 全局 Id.construct 缓存 | `Arc<str>` 直接共享 |
| 方法分发 | isinstance / if-else | `match` enum 匹配 |
| 参数类型 | 动态 float/int | `f32` only |
| 位读取 | `readBit()` 内嵌字节缓冲 | 相同，`checkBits()` 对齐 |

**关键设计决策：**

1. **静态数据与运行时分离**——`Moc2Data` 是不可变静态数据，`Send + Sync`；运行时状态（参数值、插值缓存）每个模型实例独有
2. **变形器拓扑排序在初始化时完成**——Python 每次 `init()` 重新排序，Rust 解析完一次性计算
3. **Id 使用 `Arc<str>`**——支持共享，减少内存占用
4. **PivotManager 中间结果缓存**——`ParamPivotCache` 避免每帧重复计算 param_index 绑定
5. **Lazy 顶点分配**——`DrawableState` 中的 `interpolated_vertices` 和 `transformed_vertices` 在第一次 `update()` 时分配
