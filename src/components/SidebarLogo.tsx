import markUrl from "../assets/noending-mark.svg";

export default function SidebarLogo({ size = 22, animated = false, color }: {
  size?: number;
  animated?: boolean;
  color?: string;
}) {
  return (
    <span
      role="img"
      aria-label="NoEnding"
      className={`brand-mark${animated ? " mark-reveal" : ""}`}
      style={{
        width: size * 4 / 3,
        height: size,
        backgroundColor: color,
        maskImage: `url(${markUrl})`,
        WebkitMaskImage: `url(${markUrl})`,
      }}
    />
  );
}
