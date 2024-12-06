export default function Footer() {
  return (
    <div className="z-10 pt-8 text-center text-muted-foreground">
      &copy; {new Date().getFullYear()}{" "}
      <a href="https://risczero.com" target="_blank" rel="noopener noreferrer">
        RISC Zero
      </a>{" "}
      — All rights reserved
    </div>
  );
}
