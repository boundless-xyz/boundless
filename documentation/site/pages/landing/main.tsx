import { useEffect } from "react";

export default function Main() {
  useEffect(() => {
    // redirect to the docs
    window.location.href = "/developers/why";
  }, []);

  return null;
}
