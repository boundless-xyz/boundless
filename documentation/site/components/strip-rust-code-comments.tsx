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
      return !hasHashPrefix(node.props.children);
    }
  }

  return true;
}

function processNode(node: ReactNode, index: number): ReactNode {
  if (Array.isArray(node)) {
    return node.filter((child) => shouldKeepLine(child)).map((child, i) => processNode(child, i));
  }

  if (node && typeof node === "object" && "props" in node) {
    return cloneElement(node, { ...node.props, key: index }, processNode(node.props.children, index));
  }

  return node;
}

export function StripRustCodeComments({ children }: PropsWithChildren) {
  return processNode(children, 0);
}
