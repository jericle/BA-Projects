using DataFrames

# Create a test dataframe
data = DataFrame(
    Name = ["Alice", "Bob", "Charlie", "David"],
    Department = ["Engineering", "Data", "Engineering", "Marketing"],
    Salary = [95000, 80000, 110000, 75000]
)

# Filter and calculate average
engineering_team = data[data.Department .== "Engineering", :]
avg_salary = sum(engineering_team.Salary) ÷ length(engineering_team.Salary)

println("💡 Average Engineering Salary: \$", avg_salary)
engineering_team
