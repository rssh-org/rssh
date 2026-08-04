状态：DONE

提交：518151f（feat(terminal): add pure layout tree functions）

RED：
- 命令：`npm test -- src/lib/terminal/layout.test.ts`
- 结果：失败（无法找到尚未实现的 `./layout` 模块）。

GREEN：
- 命令：`npm test -- src/lib/terminal/layout.test.ts`
- 结果：通过；1 个测试文件、6 个测试全部通过。

修改文件：
- `src/lib/terminal/layout.ts`：新增纯函数布局树类型、比例归一化、分栏插入、叶节点删除和深度优先叶 ID 收集。
- `src/lib/terminal/layout.test.ts`：新增精确需求测试，并覆盖重复 ID、未命中目标和比例边界。

自审：实现不修改输入树；重复 ID、未命中目标返回原树；删除后提升非空子树，不产生单子节点 split。未运行格式化器、lint 或项目级测试套件。
疑虑：无。
