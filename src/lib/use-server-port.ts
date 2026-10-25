import { useEffect, useState } from "react";
import { getConfig } from "./api";

/** 配置里的默认端口，拉不到时用它（服务本身也是这个默认值）。 */
export const DEFAULT_PORT = 8848;

/**
 * 当前服务端口。
 *
 * 生成调用代码时要拼进地址，**不能写死**：用户在设置页改过端口之后，
 * 界面给一份写着 8848 的示例代码，他照着跑就是连不上，而且很难想到
 * 是示例错了。
 *
 * 拉不到就退回默认值——端口只影响示例代码，不值得为它报错打扰用户。
 */
export function useServerPort(): number {
  const [port, setPort] = useState(DEFAULT_PORT);
  useEffect(() => {
    let alive = true;
    void getConfig()
      .then((c) => {
        if (alive && c.port) setPort(c.port);
      })
      .catch(() => {
        /* 浏览器预览模式拿不到配置 */
      });
    return () => {
      alive = false;
    };
  }, []);
  return port;
}
