import type React from "react";

interface OpenFluxLogoProps {
  size?: number;
  className?: string;
}

export const OpenFluxLogo: React.FC<OpenFluxLogoProps> = ({ size = 24, className }) => (
  <svg width={size} height={size} viewBox="0 0 128 128" fill="none" aria-hidden="true" className={className}>
    <path
      d="M78 24 H46 C35 24 27 31 24.5 42 L15.5 84 C13 96 21 104 33 104 H95 C107 104 115 96 112.5 84 L106.5 56"
      stroke="currentColor"
      strokeWidth={11}
      strokeLinecap="round"
      strokeLinejoin="round"
    />
    <path
      d="M41.5 86.5 C40 86.5 39 84.5 39.8 81.8 C42.8 71.5 45.2 61.5 48.2 52.5 C52.6 39.5 63.4 36.2 76.5 37.8 C84.2 38.7 90.2 35.8 93.4 34.4 C95.4 33.5 96.8 35.2 95.8 37.4 C92.4 45.0 83.2 48.2 72.5 48.0 C65.2 47.9 60.4 51.0 58.0 58.2 L57.2 60.6 C62.5 56.8 70.4 56.8 76.2 57.4 C79.4 57.7 80.5 60.2 79.4 63.0 C78.2 65.8 75.5 66.8 71.8 66.8 C64.2 66.8 58.5 68.4 54.0 74.2 C50.6 78.6 47.8 83.4 44.6 85.4 C43.5 86.1 42.5 86.5 41.5 86.5 Z"
      fill="currentColor"
    />
  </svg>
);
