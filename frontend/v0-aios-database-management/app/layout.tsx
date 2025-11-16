import type React from "react"
import type { Metadata } from "next"
import { GeistSans } from "geist/font/sans"
import { GeistMono } from "geist/font/mono"
import { Analytics } from "@vercel/analytics/next"
import { ThemeProvider } from "@/components/theme-provider"
import { AlertProvider } from "@/contexts/alert-context"
import { QueryProvider } from "@/components/providers/query-provider"
import { Suspense } from "react"
import "./globals.css"

export const metadata: Metadata = {
  title: "v0 App",
  description: "Created with v0",
  generator: "v0.app",
}

export default function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang="en" suppressHydrationWarning>
      <body className={`font-sans ${GeistSans.variable} ${GeistMono.variable}`}>
        <Suspense fallback={null}>
          <AlertProvider>
            <QueryProvider>
              <ThemeProvider attribute="class" defaultTheme="light" enableSystem disableTransitionOnChange>
                {children}
              </ThemeProvider>
            </QueryProvider>
          </AlertProvider>
          <Analytics />
        </Suspense>
      </body>
    </html>
  )
}
