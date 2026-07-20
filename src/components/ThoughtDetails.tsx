import React, { useEffect, useState } from "react";

interface ThoughtDetailsProps
  extends Omit<React.DetailsHTMLAttributes<HTMLDetailsElement>, "open"> {
  defaultOpen: boolean;
}

export const ThoughtDetails: React.FC<ThoughtDetailsProps> = ({
  defaultOpen,
  onToggle,
  ...props
}) => {
  const [open, setOpen] = useState(defaultOpen);

  useEffect(() => {
    setOpen(defaultOpen);
  }, [defaultOpen]);

  return (
    <details
      {...props}
      open={open}
      onToggle={(event) => {
        setOpen(event.currentTarget.open);
        onToggle?.(event);
      }}
    />
  );
};
