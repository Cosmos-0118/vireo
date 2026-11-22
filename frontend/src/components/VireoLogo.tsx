import React, { useId } from 'react';

interface VireoLogoProps {
  size?: number;
  className?: string;
}

const LEFT_WING_PATH =
  'M 116.05 171.34 A 16.00 16.00 0 0 1 130.27 148.00 L 180.32 148.00 A 14.00 14.00 0 0 1 193.42 157.06 L 273.84 370.36 A 16.00 16.00 0 0 1 258.87 392.00 L 239.75 392.00 A 16.00 16.00 0 0 1 225.53 383.34 Z';

const RIGHT_WING_PATH =
  'M 318.58 157.06 A 14.00 14.00 0 0 1 331.68 148.00 L 381.73 148.00 A 16.00 16.00 0 0 1 395.95 171.34 L 286.47 383.34 A 16.00 16.00 0 0 1 272.25 392.00 L 253.13 392.00 A 16.00 16.00 0 0 1 238.16 370.36 Z';

export const VireoLogo: React.FC<VireoLogoProps> = ({ size = 24, className = '' }) => {
  const uid = useId().replace(/:/g, '');
  const bgId = `vireo-bg-${uid}`;
  const rimId = `vireo-rim-${uid}`;

  return (
    <svg
      width={size}
      height={size}
      viewBox="0 0 512 512"
      fill="none"
      xmlns="http://www.w3.org/2000/svg"
      className={className}
      aria-label="Vireo"
    >
      <defs>
        <linearGradient id={bgId} x1="60" y1="20" x2="452" y2="492" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="#16D191" />
          <stop offset="55%" stopColor="#069668" />
          <stop offset="100%" stopColor="#047857" />
        </linearGradient>
        <linearGradient id={rimId} x1="256" y1="20" x2="256" y2="300" gradientUnits="userSpaceOnUse">
          <stop offset="0%" stopColor="#FFFFFF" stopOpacity="0.24" />
          <stop offset="100%" stopColor="#FFFFFF" stopOpacity="0" />
        </linearGradient>
      </defs>
      <rect x="20" y="20" width="472" height="472" rx="112" fill={`url(#${bgId})`} />
      <rect
        x="22"
        y="22"
        width="468"
        height="468"
        rx="110"
        stroke={`url(#${rimId})`}
        strokeWidth="4"
      />
      <path d={LEFT_WING_PATH} fill="#D9FBF0" fillOpacity="0.80" />
      <path d={RIGHT_WING_PATH} fill="#FFFFFF" />
    </svg>
  );
};
