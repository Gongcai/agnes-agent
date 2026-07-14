import React from "react";

interface AgentAvatarProps {
  name: string;
  avatar?: string;
  size?: number;
  className?: string;
}

/// 判断 avatar 字段是否为上传的图片（data URL）。
export function isImageAvatar(value?: string): boolean {
  return !!value && value.startsWith("data:image");
}

/// 统一的角色卡头像：图片 / emoji / 首字母 三态回退。
export const AgentAvatar: React.FC<AgentAvatarProps> = ({
  name,
  avatar,
  size = 40,
  className = "",
}) => {
  const dimension = { width: size, height: size };

  if (avatar && isImageAvatar(avatar)) {
    return (
      <img
        src={avatar}
        alt={name}
        style={dimension}
        className={`rounded-full object-cover shrink-0 ${className}`}
      />
    );
  }

  const text = avatar && !isImageAvatar(avatar) ? avatar : name?.charAt(0) || "?";
  return (
    <div
      style={dimension}
      className={`rounded-full bg-indigo-50 border border-indigo-100 flex items-center justify-center text-indigo-600 font-bold shrink-0 select-none ${className}`}
    >
      <span style={{ fontSize: size * 0.5, lineHeight: 1 }}>{text}</span>
    </div>
  );
};
