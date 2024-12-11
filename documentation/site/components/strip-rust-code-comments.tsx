import { type PropsWithChildren, type ReactNode, cloneElement } from "react";

function hasHashPrefix(node: ReactNode): boolean {
  if (typeof node === "string") {
    return node.trimStart().startsWith("#");
  }

  if (Array.isArray(node)) {
    return node.some((child) => hasHashPrefix(child));
  }

  if (node && typeof node === "object" && "props" in node) {
    return hasHashPrefix(node.props.children);
  }

  return false;
}

function shouldKeepLine(node: ReactNode): boolean {
  if (node && typeof node === "object" && "props" in node) {
    if (node.props.className?.includes("line")) {
      // Check recursively if any children contain `#`
      return !hasHashPrefix(node.props.children);
    }
  }

  return true;
}

function processNode(node: ReactNode): ReactNode {
  if (Array.isArray(node)) {
    return node.filter((child) => shouldKeepLine(child)).map((child) => processNode(child));
  }

  // If it's a React element
  if (node && typeof node === "object" && "props" in node) {
    // Clone the element with processed children
    return cloneElement(node, node.props, processNode(node.props.children));
  }

  return node;
}

// allows you to strip out comments from a Rust code block
// any line prefixed with `#` will be stripped out
export function StripRustCodeComments({ children }: PropsWithChildren) {
  return <>{processNode(children)}</>;
}
