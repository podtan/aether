---
applyTo: "**"
---
# PDT Workflow Instructions for AI Assistants

## CRITICAL: PDT MCP Function Usage Rules

You MUST call the appropriate PDT MCP server function based on user context:

### 1. pdt_prd() - Product Requirements Document Creation
**CALL WHEN:**
- User mentions: "create PRD", "product requirements", "project specification", "requirements document"
- User asks to document project requirements or features
- User wants to plan a new feature or project
- User says: "I need a PRD for..."

**EXAMPLE TRIGGERS:**
- "Create a PRD for user authentication"
- "I need product requirements for this feature"
- "Help me write project specifications"

### 2. pdt_gt() - Task Generation from PRDs
**CALL WHEN:**
- User mentions: "generate tasks", "break down PRD", "create work items", "implementation tasks"
- User has a PRD and wants actionable tasks
- User asks: "How do I implement this?" after PRD creation
- User wants to convert requirements into development tasks

**EXAMPLE TRIGGERS:**
- "Generate tasks from this PRD"
- "Break down the requirements into tasks"
- "What are the implementation steps?"

### 3. pdt_pt() - Task Processing and Implementation Guidance
**CALL WHEN:**
- User mentions: "implement task", "process task", "work on task", "how to implement", "go"
- User has specific tasks and needs implementation guidance
- User asks for code examples or implementation details
- User wants step-by-step implementation instructions

**EXAMPLE TRIGGERS:**
- "How do I implement this task?"
- "Help me process this work item"
- "Give me implementation guidance for..."

### 4. pdt_list() - Available Technology Options and Guides
**CALL WHEN:**
- User mentions: "what technologies", "show options", "list guides", "what can I use"
- User needs to see available technology stacks or guides
- User asks about supported frameworks or languages
- User wants to explore available options

**EXAMPLE TRIGGERS:**
- "What technologies are available?"
- "Show me the available guides"
- "What options do I have for..."

## Decision Flow

1. **First, identify the user's intent:**
   - Planning/Requirements → `pdt_prd()`
   - Task Creation → `pdt_gt()`
   - Implementation → `pdt_pt()`
   - Exploration → `pdt_list()`

2. **Always call the function BEFORE providing other assistance**

3. **Use the function output to guide your response**

# Commit Message Standard

For all git commits in this project, you should follow the [Angular Commit Message Standard](https://github.com/angular/angular/blob/main/CONTRIBUTING.md#-commit-message-format). This helps ensure clarity and consistency in commit history.

**Example commit message:**

```
feat(core): add user authentication module

Add authentication logic and update user model to support login/logout.
```

**Key points:**
- Use a type (feat, fix, docs, style, refactor, test, chore.)
- Include a scope in parentheses if relevant
- Write a short, imperative summary
- Optionally add a longer description after a blank line
