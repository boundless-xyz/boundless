import { type PropsWithChildren, type ReactNode, cloneElement } from "react";

type Props = PropsWithChildren;

function hasHashPrefix(node: ReactNode): boolean {
  // If it's a string, check if it starts with #
  if (typeof node === "string") {
    return node.trimStart().startsWith("#");
  }

  // If it's an array, check each child
  if (Array.isArray(node)) {
    return node.some((child) => hasHashPrefix(child));
  }

  // If it's a React element, check its children
  if (node && typeof node === "object" && "props" in node) {
    return hasHashPrefix(node.props.children);
  }

  return false;
}

function shouldKeepLine(node: ReactNode): boolean {
  // If it's a React element with className "line"
  if (node && typeof node === "object" && "props" in node) {
    if (node.props.className?.includes("line")) {
      // Check recursively if any children contain #
      return !hasHashPrefix(node.props.children);
    }
  }

  return true;
}

function processNode(node: ReactNode): ReactNode {
  // If it's an array, process each child
  if (Array.isArray(node)) {
    return node.filter((child) => shouldKeepLine(child)).map((child) => processNode(child));
  }

  // If it's a React element
  if (node && typeof node === "object" && "props" in node) {
    // Clone the element with processed children
    return cloneElement(node, node.props, processNode(node.props.children));
  }

  // Return unchanged for other types
  return node;
}

export function StripRustCode({ children }: Props) {
  return <>{processNode(children)}</>;
}
