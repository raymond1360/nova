// Vue host renderer for component event/reactivity tests in Vitest's Node
// environment. This is not a browser, pixel, native-input or Tauri runtime test.
import { createRenderer, nextTick, type Component } from 'vue'
import type { Pinia } from 'pinia'

export class TestNode extends EventTarget {
  children: TestNode[] = []
  parent: TestNode | null = null
  props: Record<string, any> = {}
  text = ''
  value: any = ''
  _value: any
  selected = false
  checked = false
  composing = false
  constructor(public kind: string) { super() }
  getRootNode(): TestNode { return this.parent?.getRootNode() ?? this }
  get tagName() { return this.kind.toUpperCase() }
  get type() { return this.props.type ?? 'text' }
  get multiple() { return !!this.props.multiple }
  get options(): TestNode[] { return this.children.flatMap(c => c.kind === 'option' ? [c] : c.options) }
  get selectedIndex() { return this.options.findIndex(o => o.selected) }
  set selectedIndex(index: number) { this.options.forEach((o, i) => { o.selected = i === index }) }
  get textContent(): string { return this.text + this.children.map(c => c.textContent).join('') }
}

const renderer = createRenderer<TestNode, TestNode>({
  createElement: tag => new TestNode(tag),
  createText: text => Object.assign(new TestNode('#text'), { text }),
  createComment: text => Object.assign(new TestNode('#comment'), { text: '' }),
  setText: (node, text) => { node.text = text },
  setElementText: (node, text) => { node.text = text; node.children = [] },
  parentNode: node => node.parent,
  nextSibling: node => node.parent?.children[node.parent.children.indexOf(node) + 1] ?? null,
  insert(node, parent, anchor = null) {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1)
    node.parent = parent
    const index = anchor ? parent.children.indexOf(anchor) : -1
    if (index < 0) parent.children.push(node)
    else parent.children.splice(index, 0, node)
  },
  remove(node) {
    if (node.parent) node.parent.children.splice(node.parent.children.indexOf(node), 1)
    node.parent = null
  },
  patchProp(node, key, _previous, value) {
    node.props[key] = value
    if (key === 'value') { node.value = value; node._value = value }
  },
})

export function renderComponent(component: Component, pinia: Pinia) {
  const root = new TestNode('root')
  const app = renderer.createApp(component)
  app.use(pinia)
  app.mount(root)
  return { root, unmount: () => app.unmount() }
}

export function all(root: TestNode, predicate: (node: TestNode) => boolean): TestNode[] {
  return [...(predicate(root) ? [root] : []), ...root.children.flatMap(c => all(c, predicate))]
}

export function button(root: TestNode, text: string): TestNode {
  const found = all(root, n => n.kind === 'button' && n.textContent.trim() === text)
  if (found.length !== 1) throw new Error(`Expected one button: ${text}; found ${found.length}`)
  return found[0]
}

export async function settle() {
  for (let i = 0; i < 20; i++) await Promise.resolve()
  await nextTick()
}

export async function click(node: TestNode) {
  if (node.props.disabled) throw new Error('Cannot click disabled test node')
  await node.props.onClick(new Event('click'))
  await settle()
}

export async function input(node: TestNode, text: string) {
  node.value = text
  node.dispatchEvent(new Event('input')) // exercises Vue's actual v-model directive
  await settle()
}
