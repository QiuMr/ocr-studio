import * as React from "react";
import { Slot } from "@radix-ui/react-slot";
import { cva, type VariantProps } from "class-variance-authority";
import { cn } from "../../lib/utils";

/**
 * shadcn/ui 风格按钮。规范：**每个按钮都必须带图标**（lucide-react），
 * 因此默认变体预留了图标插槽，调用方用 `<Button><Icon /> 文字</Button>`。
 *
 * 2026-10-22 尺寸统一（界面精修）：高度只有四档，字号跟着档位走，
 * 圆角一律取 `rounded-md`（=8px，见 styles.css 的圆角梯度）。
 *
 *   xs      h-7 (28px)  text-12  密集行内动作（工具条、卡片角上的小按钮）
 *   sm      h-8 (32px)  text-12  次级按钮（表单旁、卡片头）
 *   default h-9 (36px)  text-13  主按钮
 *   lg      h-10 (40px) text-13  强调按钮（落地页/空状态）
 *   icon*   正方形，跟同档位按钮等高
 *
 * 图标统一 14px（调用方写 `className="h-3.5 w-3.5"`），与 12~13px 文字对齐。
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
        xs: "h-7 px-2.5 text-[12px]",
        sm: "h-8 px-3 text-[12px]",
        default: "h-9 px-4 py-2",
        lg: "h-10 px-6",
        icon: "h-9 w-9",
        "icon-sm": "h-8 w-8",
        "icon-xs": "h-7 w-7",
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
