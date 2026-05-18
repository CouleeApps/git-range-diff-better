# git-range-diff-better

Compare diff before/after a rebase, except better than `git range-diff` because it handles submodules
and doesn't break horribly on rebase squashes.

## Usage

```shell
cd my-git-repo
git-range-diff-better b7c14457a3d4620d2e8f9f1b03bd327f8b5a2510..5f770fe793dc4296daf9a7a89fbd59992fdc043a eb9f7c2499f9461dc3beaa9908bb1bfbbaba57f2..71a184c0806687f0f9a630ea4ea14c7853cf5ad4
```

## Sample Output

It's a bit tricky to read, but once you figure out the first column of +/- is the diff-of-diffs
and the second column is the changed diff, it gets a bit easier. There are these states to consider:

- `+ +`: New changes made by the rebase have added some code
- `+ -`: New changes made by the rebase have removed some code
- `- +`: The rebase caused some previously added code to no longer be added
- `- -`: The rebase caused some previously removed code to no longer be removed
- `  +`: Both diffs are the same (rebase did not change anything)
- `  -`: Both diffs are the same (rebase did not change anything)

Here's a dump of a public binja rebase change. You'll have to pretend there's a few lines at the top
for the private changes in the root repo.

```diff
  diff --git a/api b/api
  index 3021a6889..e49af66d1 160000
  --- a/api
  +++ b/api
  @@ -1 +1 @@
- -Subproject commit 3021a6889ba7bd36ffd2dd2ce5000284c2d95318
- +Subproject commit e49af66d144962fc6a0624322d123d36c8546c59
+ -Subproject commit f1a688a1e7fb6d962d8c9addaaefdfd59e5dd28b
+ +Subproject commit 3edde1ca6b4ed462c10407b551715f6b398c3470
  diff --git a/ui/shared/scriptingconsole.cpp b/ui/shared/scriptingconsole.cpp
  index e13f4e50c..b5c6a4bbc 100644
  --- a/ui/shared/scriptingconsole.cpp
  <snip: private code changes of a style and format similar to the public changes below>
Submodule api
  diff --git a/binaryninjaapi.h b/binaryninjaapi.h
  index d9e1efc06..9c6f76346 100644
  --- a/binaryninjaapi.h
   		virtual std::string CompleteInput(const std::string& text, uint64_t state);
   		virtual void Stop();
  +		virtual bool CanCompleteArguments(const std::string& text);
- +		virtual std::string CompleteArguments(const std::string& text, uint64_t* argumentStart);
+ +		virtual std::pair<std::string, uint64_t> CompleteArguments(const std::string& text);

   		void Output(const std::string& text);
   		void Warning(const std::string& text);
   		virtual std::string CompleteInput(const std::string& text, uint64_t state) override;
   		virtual void Stop() override;
  +		virtual bool CanCompleteArguments(const std::string& text) override;
- +		virtual std::string CompleteArguments(const std::string& text, uint64_t* argumentStart) override;
+ +		virtual std::pair<std::string, uint64_t> CompleteArguments(const std::string& text) override;
   	};

   	/*!
  diff --git a/scriptingprovider.cpp b/scriptingprovider.cpp
  +char* ScriptingInstance::CompleteArgumentsCallback(void* ctx, const char* text, uint64_t* argumentStart)
  +{
  +	CallbackRef<ScriptingInstance> instance(ctx);
- +	auto result = instance->CompleteArguments(text, argumentStart);
- +	return BNAllocString(result.c_str());
+ +	auto result = instance->CompleteArguments(text);
+ +	if (argumentStart)
+ +		*argumentStart = result.second;
+ +	return BNAllocString(result.first.c_str());
  +}
  +
  +
  +}
  +
  +
- +std::string ScriptingInstance::CompleteArguments(const std::string&, uint64_t* argumentStart)
+ +std::pair<std::string, uint64_t> ScriptingInstance::CompleteArguments(const std::string&)
  +{
- +	if (argumentStart)
- +		*argumentStart = 0;
- +	return "";
+ +	return {"", 0};
  +}
  +
  +
  +}
  +
  +
- +std::string CoreScriptingInstance::CompleteArguments(const std::string& text, uint64_t* argumentStart)
+ +std::pair<std::string, uint64_t> CoreScriptingInstance::CompleteArguments(const std::string& text)
  +{
- +	char* result = BNScriptingInstanceCompleteArguments(m_object, text.c_str(), argumentStart);
+ +	uint64_t argumentStart = 0;
+ +	char* result = BNScriptingInstanceCompleteArguments(m_object, text.c_str(), &argumentStart);
  +	if (!result)
- +		return "";
+ +		return {"", argumentStart};
  +	std::string ret = result;
  +	BNFreeString(result);
- +	return ret;
+ +	return {ret, argumentStart};
  +}
  +
  +
   void CoreScriptingInstance::Stop()
   {
   	BNStopScriptingInstance(m_object);
```

## AI Disclosure
This repo was entirely vibe-coded by GPT-5.5 medium. No guarantees are made to its code quality.
