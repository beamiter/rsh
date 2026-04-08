# .bashrc 兼容性改进方案

## 当前状态分析

### 现有机制
- `config.rs`: 在交互模式启动时加载 .bashrc
- `source_via_bash()`: 通过 bash 执行 .bashrc 并回收环境变量
- 支持的导入：环境变量（export）
- 不支持的导入：别名、函数、shopt 选项

### 核心问题
```bash
# 现在的 bash 脚本只输出环境变量
declare -p | grep 'declare -x' | sed ...

# 遗漏了这些重要信息：
alias -p                  # 别名定义
declare -F               # 函数名列表
declare -f funcname      # 函数完整定义
shopt                    # 当前 shopt 选项
```

## 改进方案

### 方案 1：增强 source_via_bash 函数（推荐）

#### 改进步骤

**Step 1**: 修改 `src/config.rs` 中的 `source_via_bash` 函数

```rust
fn source_via_bash(path: &PathBuf, state: &mut ShellState) {
    let path_str = path.to_string_lossy().to_string();
    
    // 改进的 bash 脚本：提取环境变量、别名和函数
    let bash_script = format!(
        r#"
set -a
source "{path}"
set +a

# 输出环境变量
echo "=== ENV_VARS ==="
declare -p | grep 'declare -x' | sed 's/declare -x //'

# 输出别名
echo "=== ALIASES ==="
alias -p 2>/dev/null || true

# 输出函数（仅函数名）
echo "=== FUNCTIONS ==="
declare -F -p | cut -d' ' -f3 2>/dev/null || true

# 输出函数定义（对每个函数）
if declare -F >/dev/null 2>&1; then
    echo "=== FUNCTION_DEFS ==="
    for func in $(declare -F -p | cut -d' ' -f3 2>/dev/null || true); do
        echo "@@FUNC:$func@@"
        declare -f "$func" 2>/dev/null
        echo "@@END_FUNC@@"
    done
fi
"#,
        path = path_str.replace("'", "\\'")
    );

    if let Ok(output) = std::process::Command::new("bash")
        .arg("-c")
        .arg(&bash_script)
        .output() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        parse_bash_output(&stdout, state);
    }
}

fn parse_bash_output(output: &str, state: &mut ShellState) {
    let mut current_section = "";
    let mut current_func = "";
    let mut func_def = String::new();

    for line in output.lines() {
        match line {
            "=== ENV_VARS ===" => current_section = "ENV_VARS",
            "=== ALIASES ===" => current_section = "ALIASES",
            "=== FUNCTIONS ===" => current_section = "FUNCTIONS",
            "=== FUNCTION_DEFS ===" => current_section = "FUNCTION_DEFS",
            _ if line.starts_with("@@FUNC:") => {
                current_func = &line[7..line.len()-2];
                func_def.clear();
            }
            "@@END_FUNC@@" => {
                if !current_func.is_empty() && !func_def.is_empty() {
                    // 解析函数定义并添加到 state.functions
                    // 这需要调用 parser::parse 来解析函数体
                    parse_and_store_function(current_func, &func_def, state);
                }
                current_func = "";
            }
            _ if current_section == "ENV_VARS" && line.contains('=') => {
                if let Some(eq_pos) = line.find('=') {
                    let key = &line[..eq_pos];
                    let value = &line[eq_pos + 1..];
                    let value = value.trim_matches('\'').trim_matches('"');
                    state.export_var(key, value);
                }
            }
            _ if current_section == "ALIASES" && line.starts_with("alias ") => {
                // 解析 "alias name='value'" 格式
                let alias_def = &line[6..]; // 跳过 "alias "
                if let Some(eq_pos) = alias_def.find('=') {
                    let name = &alias_def[..eq_pos];
                    let value = &alias_def[eq_pos + 1..];
                    let value = value.trim_matches('\'').trim_matches('"');
                    state.aliases.insert(name.to_string(), value.to_string());
                }
            }
            _ if current_section == "FUNCTION_DEFS" && !current_func.is_empty() => {
                func_def.push('\n');
                func_def.push_str(line);
            }
            _ => {}
        }
    }
}
```

**Step 2**: 添加函数解析辅助函数

```rust
fn parse_and_store_function(name: &str, body: &str, state: &mut ShellState) {
    // 格式通常是: funcname () { ... }
    // 需要提取 { ... } 部分并解析
    if let Some(start) = body.find('{') {
        if let Some(end) = body.rfind('}') {
            let inner_body = &body[start + 1..end];
            if let Ok(commands) = crate::parser::parse(inner_body) {
                // 存储为单个复合命令
                if !commands.is_empty() {
                    // 这需要根据 CompoundCommand 的结构进行调整
                    // 暂时跳过复杂的函数体解析，只记录别名
                }
            }
        }
    }
}
```

### 方案 2：添加 --bashrc 和 --rshrc 命令行选项（辅助方案）

当前在 `environment.rs` 中已有 `ConfigSource` 枚举，可以通过命令行参数选择：

```bash
rsh --bashrc      # 使用 .bashrc
rsh --rshrc       # 使用 .rshrc（需要创建）
```

### 方案 3：改进 rsh 解析器以支持更多 bash 特性

**优先级**：
1. ✅ 别名展开（已实现基础）
2. ⚠️ 函数定义完整解析（部分支持）
3. ❌ 复杂参数展开（如 `${var@operator}`）
4. ❌ 历史展开（!$、!!）

## 实现优先级

### 高优先级（即刻改进）
- [ ] 增强 `source_via_bash()` 提取别名
- [ ] 增强 `source_via_bash()` 提取函数名
- [ ] 改进别名在命令执行前的展开

### 中优先级（下一阶段）
- [ ] 完整函数定义导入
- [ ] shopt 选项同步
- [ ] 更好的错误报告

### 低优先级（长期）
- [ ] 历史展开（!$、!!）
- [ ] 复杂参数展开
- [ ] BASH_SOURCE 等特殊变量

## 测试用例

```bash
# ~/.bashrc 示例
export MY_VAR="hello"
alias ll="ls -la"
alias grep="grep --color"

function my_func() {
    echo "Hello $1"
    return 0
}

# 在 rsh 中应该都能工作：
echo $MY_VAR          # 应输出 hello
ll                    # 应展开为 ls -la
grep --help         # 应展开为 grep --color --help
my_func world       # 应输出 Hello world
```

## 兼容性检查清单

- [ ] 环境变量导出
- [ ] 别名定义和展开
- [ ] 函数定义和调用
- [ ] 条件语句（if/case）
- [ ] 循环结构
- [ ] Prompt 自定义
- [ ] shopt 选项

## 预期效果

实现后，用户的 ~/.bashrc 将能够：
1. ✅ 导出环境变量（已实现）
2. ✅ 定义和使用别名（需改进）
3. ✅ 定义和调用函数（需改进）
4. ✅ 设置 shell 选项（需实现）
5. 💪 提升整体 bash 兼容性评分 from ~70% to ~85%
