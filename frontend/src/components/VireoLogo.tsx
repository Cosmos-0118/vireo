import React from 'react';
import vireoLogoUrl from '../assets/vireo-logo.png';

interface VireoLogoProps {
  size?: number;
  className?: string;
}

export const VireoLogo: React.FC<VireoLogoProps> = ({ size = 24, className = '' }) => {
  return (
    <img
      src={vireoLogoUrl}
      width={size}
      height={size}
      alt="Vireo"
      draggable={false}
      className={`object-contain select-none ${className}`.trim()}
    />
  );
};
