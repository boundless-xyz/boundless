export default function Footer() {
	return (
		<div className="text-muted-foreground text-center pt-8">
			&copy; {new Date().getFullYear()}{" "}
			<a href="https://risczero.com" target="_blank" rel="noopener noreferrer">
				RISC Zero
			</a>{" "}
			— All rights reserved
		</div>
	);
}
