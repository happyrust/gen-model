"use client"

import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card"
import { Sidebar } from "@/components/sidebar"

export default function UsersPage() {
  const users = [
    { id: 1, name: "Alice", email: "alice@example.com" },
    { id: 2, name: "Bob", email: "bob@example.com" },
    { id: 3, name: "Charlie", email: "charlie@example.com" },
  ]

  return (
    <div className="min-h-screen bg-background">
      <Sidebar />
      <div className="ml-64 p-8">
        <Card>
          <CardHeader>
            <CardTitle>Users</CardTitle>
          </CardHeader>
          <CardContent>
            <ul>
              {users.map((user) => (
                <li key={user.id}>
                  {user.name} - {user.email}
                </li>
              ))}
            </ul>
          </CardContent>
        </Card>
      </div>
    </div>
  )
}
