import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../../lib/utils";

/**
 * shadcn/ui 风格按钮。规范：**每个按钮都必须带图标**（lucide-react），
 * 因此默认变体预留了图标插槽，调用方用 `<Button><Icon /> 文字</Button>`。
 */
const buttonVariants = cva(
  "inline-flex items-center justify-center gap-2 whitespace-nowrap rounded-md text-sm font-medium transition-colors focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-blue-500 focus-visible:ring-offset-2 disabled:pointer-events-none disabled:opacity-50 [&_svg]:pointer-events-none [&_svg]:shrink-0",
  {
    variants: {
      variant: {
        // 主色：蓝（点缀色只给主动作）
        default: "bg-blue-600 text-white shadow-sm hover:bg-blue-700",
        destructive: "bg-red-600 text-white shadow-sm hover:bg-red-700",
        // 次级：白底黑字（纯白为主、黑色为辅）
        outline:
          "border border-neutral-200 bg-white text-neutral-900 shadow-sm hover:bg-neutral-50",
        secondary: "bg-neutral-100 text-neutral-900 hover:bg-neutral-200",
        ghost: "text-neutral-700 hover:bg-neutral-100",
        link: "text-blue-600 underline-offset-4 hover:underline",
      },
      size: {
        default: "h-9 px-4 py-2",
        sm: "h-8 rounded-md px-3 text-xs",
        lg: "h-10 rounded-md px-6",
        icon: "h-9 w-9",
      },
    },
    defaultVariants: {
      variant: "default",
      size: "default",
    },
  },
);

export interface ButtonProps
  extends React.ButtonHTMLAttributes<HTMLButtonElement>,
    VariantProps<typeof buttonVariants> {
  asChild?: boolean;
}

const Button = React.forwardRef<HTMLButtonElement, ButtonProps>(
  ({ className, variant, size, asChild = false, ...props }, ref) => {
    const Comp = asChild ? Slot : "button";
    return (
      <Comp
        className={cn(buttonVariants({ variant, size, className }))}
        ref={ref}
        {...props}
      />
    );
  },
);
Button.displayName = "Button";

export { Button, buttonVariants };
