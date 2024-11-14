export default function RootLayout({ children }) {
	return (
		<>
			{/* Custom JS scripts */}
			<script
				defer
				data-domain="docs.beboundless.xyz"
				src="https://plausible.io/js/script.outbound-links.js"
			/>

			{children}
		</>
	);
}
