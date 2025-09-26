export default function Footer() {
  return (
    <div className="z-10 pt-8 text-center">
      &copy; {new Date().getFullYear()}{" "}
      <a href="https://boundless.network" target="_blank" rel="noopener noreferrer">
        Boundless Networks
      </a>{" "}
      — All rights reserved
    </div>
  );
}
